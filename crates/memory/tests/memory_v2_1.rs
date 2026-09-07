use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use axum::{Json, Router, extract::State as AxumState, http::StatusCode, routing::post};
use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_core::{CommitPhase, FailureInjector, VaultCore};
use mcp_vault_domain::{
    Actor, EmbeddingId, MemoryId, MemorySourceId, ModelId, ProviderId, Revision, SourcePlane,
    VaultContext, VaultId, VaultPath, VaultSlug,
};
use mcp_vault_memory::{
    ExtractionPolicy, MemoryError, MemoryOrigin, MemoryOwnership, MemoryService, MemoryType,
    MemoryUpdateInput, NoteExtractionOptions, RecallContext, RecallRequest, RememberInput,
};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderMode,
    ProviderService, ProviderSettings,
};
use mcp_vault_state::{
    EmbeddingRecord, MemoryBundle, MemoryRecord, MemorySourceRecord, ModelRecord, ProviderRecord,
    StateStore, VaultStatus,
};
use mcp_vault_storage_fs::StorageOptions;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::sync::Notify;

const MODEL_NORMAL: usize = 0;
const MODEL_INVALID_ROOT: usize = 1;
const MODEL_EMPTY_SET: usize = 2;
const MODEL_BLOCKED: usize = 3;
const MODEL_CHAIN: usize = 5;
const MODEL_JUDGMENT_BLOCKED: usize = 6;
const MODEL_ALL_DIFFERENT: usize = 7;
const MODEL_INVALID_PAIR: usize = 8;
const MODEL_SENTENCE_BLOCKED: usize = 9;

struct FailOnceAt {
    phase: CommitPhase,
    fired: AtomicBool,
}

impl FailOnceAt {
    fn new(phase: CommitPhase) -> Self {
        Self {
            phase,
            fired: AtomicBool::new(false),
        }
    }
}

impl FailureInjector for FailOnceAt {
    fn fail(&self, phase: CommitPhase) -> Result<(), &'static str> {
        if phase == self.phase && !self.fired.swap(true, Ordering::SeqCst) {
            Err("deterministic memory snapshot recovery fault")
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
struct CurrentSetModelState {
    calls: AtomicUsize,
    mode: AtomicUsize,
    started: Notify,
    release: Notify,
    requests: tokio::sync::Mutex<Vec<Value>>,
}

async fn current_set_model(
    AxumState(state): AxumState<Arc<CurrentSetModelState>>,
    Json(request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let system = request["messages"][0]["content"]
        .as_str()
        .unwrap_or_default();
    let user = request["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    let schema = &request["response_format"]["json_schema"]["schema"];
    if system.starts_with("Remove only repeated or paraphrased sentences") {
        state.requests.lock().await.push(request.clone());
        state.calls.fetch_add(1, Ordering::SeqCst);
        if state.mode.load(Ordering::SeqCst) == MODEL_SENTENCE_BLOCKED {
            state.started.notify_one();
            state.release.notified().await;
        }
        let body: Value = serde_json::from_str(user).unwrap();
        return (
            StatusCode::OK,
            Json(
                json!({"choices":[{"message":{"content":json!({"content":body["content"]}).to_string()}}]}),
            ),
        );
    }
    if system.starts_with("Compare two untrusted source-owned memory propositions.") {
        state.requests.lock().await.push(request.clone());
        state.calls.fetch_add(1, Ordering::SeqCst);
        let pair: Value = serde_json::from_str(user).unwrap();
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/memory-quality/dedup-merge.json"
        ))
        .unwrap();
        let mut relation = corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| {
                c["left"] == pair["left"]["content"] && c["right"] == pair["right"]["content"]
            })
            .and_then(|c| c["relation"].as_str())
            .unwrap_or(if pair["left"]["content"] == pair["right"]["content"] {
                "equivalent"
            } else {
                "different"
            });
        if state.mode.load(Ordering::SeqCst) == MODEL_CHAIN {
            let a = pair["left"]["content"].as_str().unwrap();
            let b = pair["right"]["content"].as_str().unwrap();
            relation = if (a.contains("1.94") && b.contains("Zig"))
                || (b.contains("1.94") && a.contains("Zig"))
            {
                "different"
            } else {
                "equivalent"
            };
        }
        if state.mode.load(Ordering::SeqCst) == MODEL_ALL_DIFFERENT {
            relation = "different";
        }
        if state.mode.load(Ordering::SeqCst) == MODEL_JUDGMENT_BLOCKED {
            state.started.notify_one();
            state.release.notified().await;
        }
        return (
            StatusCode::OK,
            Json(json!({"choices":[{"message":{"content":
                json!({"left":if state.mode.load(Ordering::SeqCst)==MODEL_INVALID_PAIR {2}else{0},"right":1,"relation":relation}).to_string()
            }}]})),
        );
    }
    if !system.contains("complete set of durable, useful memories")
        || system.contains("Phase 1")
        || system.contains("consolidat")
        || schema["properties"]
            .as_object()
            .map(|properties| properties.len())
            != Some(1)
        || schema["properties"].get("memories").is_none()
    {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "unexpected current-set extraction contract"})),
        );
    }
    state.requests.lock().await.push(request.clone());
    state.calls.fetch_add(1, Ordering::SeqCst);
    let mode = state.mode.load(Ordering::SeqCst);
    if mode == MODEL_INVALID_ROOT {
        return (
            StatusCode::OK,
            Json(json!({
                "choices": [{"message": {"content": "{\"items\":[]}"}}]
            })),
        );
    }
    if mode == MODEL_BLOCKED {
        state.started.notify_one();
        state.release.notified().await;
    }
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/memory-quality/dedup-merge.json"
    ))
    .unwrap();
    let fixture_case = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| user.contains(&format!("DEDUP_CASE {}:", case["id"].as_str().unwrap())));
    let memories = if let Some(marker) = user.split("FAIRNESS:").nth(1) {
        let label: String = marker.chars().take_while(char::is_ascii_digit).collect();
        json!([{"content":format!("Project {label} uses Rust. Batch size is 32. Go was not adopted."),"kind":"fact"}])
    } else if user.contains("DEDUP_CONTAINED") {
        let case = &corpus["cases"][48];
        json!([{"content":case["left"],"kind":"fact"},{"content":case["right"],"kind":"fact"}])
    } else if let Some(case) = fixture_case {
        let side = if user.contains(&format!("DEDUP_CASE {}:left", case["id"].as_str().unwrap())) {
            "left"
        } else {
            "right"
        };
        json!([{"content":case[side],"kind":"fact"}])
    } else if mode == MODEL_EMPTY_SET {
        json!([])
    } else if user.contains("THIRD") {
        json!([
            {"content": "The Alpha team now requires Zig 0.15 for backend builds.", "kind": "decision", "tags": ["backend"]}
        ])
    } else if user.contains("SECOND") {
        json!([
            {"content": "The Alpha team now requires Rust 1.95 for backend builds.", "kind": "decision", "tags": ["backend"]}
        ])
    } else {
        json!([
            {"content": "The Alpha team requires Rust 1.94 only for backend builds.", "kind": "decision", "tags": ["backend"]},
            {"content": "The proposed switch to Go was not adopted by the Alpha team.", "kind": "fact", "tags": ["non-adoption"]}
        ])
    };
    (
        StatusCode::OK,
        Json(json!({
            "choices": [{"message": {"content": json!({"memories": memories}).to_string()}}]
        })),
    )
}

async fn fixture(slug: &str) -> (TempDir, StateStore, VaultContext, VaultCore, MemoryService) {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new(slug).unwrap(),
        directory.path().join("vault"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&context, slug, VaultStatus::Active)
        .await
        .unwrap();
    let core = VaultCore::new(
        state.clone(),
        directory.path().join("history"),
        Default::default(),
        StorageOptions::default(),
        Default::default(),
    );
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[23_u8; 32]).unwrap(),
    );
    let service = MemoryService::new(state.clone(), auth);
    (directory, state, context, core, service)
}

async fn configure_extraction(
    state: &StateStore,
    context: &VaultContext,
    service: &MemoryService,
    model_state: Arc<CurrentSetModelState>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(current_set_model))
                .with_state(model_state),
        )
        .await
        .unwrap();
    });
    let providers = ProviderService::new(
        state.clone(),
        AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[23_u8; 32]).unwrap(),
        ),
    );
    providers
        .set_provider_mode(context, ProviderMode::LocalOnly, None)
        .await
        .unwrap();
    let provider = providers
        .create_provider(ProviderInput {
            name: "current-set-model".to_owned(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: url::Url::parse(&format!("http://{address}/v1/")).unwrap(),
            settings: ProviderSettings::default(),
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();
    let model = providers
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "current-set-model".to_owned(),
            capabilities: ModelCapabilities {
                structured_output: true,
                max_output_tokens: Some(8_192),
                ..ModelCapabilities::default()
            },
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await
        .unwrap();
    providers
        .bind_model(
            Some(context),
            "memory_extraction",
            model.id,
            json!({}),
            None,
        )
        .await
        .unwrap();
    service
        .set_extraction_policy(
            context,
            ExtractionPolicy {
                enabled: true,
                ..ExtractionPolicy::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
}

async fn seed_vector_model(state: &StateStore) -> (ProviderId, ModelId) {
    let provider_id = ProviderId::new();
    state
        .providers()
        .insert_provider(&ProviderRecord {
            id: provider_id,
            name: "vector-cleanup-test".to_owned(),
            provider_type: "fastembed_local".to_owned(),
            base_url: "http://127.0.0.1/".to_owned(),
            secret_id: None,
            settings: json!({}),
            enabled: true,
            revision: Revision::new(1),
            embedding_revision: Revision::new(1),
            created_at: 1,
            updated_at: 1,
        })
        .await
        .unwrap();
    let model_id = ModelId::new();
    state
        .providers()
        .insert_model(&ModelRecord {
            id: model_id,
            provider_id,
            external_model_id: "vector-cleanup-test".to_owned(),
            capabilities: json!({"embeddings": true, "dimension": 2}),
            settings: json!({}),
            enabled: true,
            revision: Revision::new(1),
            embedding_revision: Revision::new(1),
            created_at: 1,
            updated_at: 1,
        })
        .await
        .unwrap();
    (provider_id, model_id)
}

async fn seed_memory_vector(
    state: &StateStore,
    context: &VaultContext,
    provider_id: ProviderId,
    model_id: ModelId,
    memory_id: MemoryId,
    content_hash: &str,
) {
    state
        .providers()
        .upsert_embedding(
            context,
            &EmbeddingRecord {
                id: EmbeddingId::new(),
                vault_id: context.id(),
                object_type: "memory".to_owned(),
                object_id: memory_id.to_string(),
                chunk_key: "body:0000".to_owned(),
                provider_id,
                model_id,
                dimension: 2,
                content_hash: content_hash.to_owned(),
                profile_hash: "test-profile".to_owned(),
                input_hash: "test-input".to_owned(),
                vector_backend_key: format!("{}:memory:{memory_id}:body:0000", context.id()),
                created_at: 1,
                updated_at: 1,
            },
            &[1.0, 0.5],
        )
        .await
        .unwrap();
}

async fn seed_legacy_memory(
    state: &StateStore,
    context: &VaultContext,
    id: MemoryId,
    status: &str,
    content: &str,
    sources: Vec<MemorySourceRecord>,
) {
    state
        .memory()
        .replace_bundle(
            context,
            &MemoryBundle {
                memory: MemoryRecord {
                    id,
                    vault_id: context.id(),
                    memory_type: "decision".to_owned(),
                    status: status.to_owned(),
                    status_reason: None,
                    status_changed_at: None,
                    content: content.to_owned(),
                    normalized_content: content.to_lowercase(),
                    content_hash: format!("sha256:legacy-{id}"),
                    importance: 0.83,
                    confidence: 0.77,
                    origin: "explicit_agent".to_owned(),
                    revision: Revision::new(4),
                    canonical_file_id: None,
                    canonical_path: None,
                    canonical_revision: None,
                    valid_from: Some(1_700_000_000_000),
                    valid_to: Some(1_900_000_000_000),
                    extraction: json!({"legacy": true}),
                    created_at: 1_700_000_000_000,
                    updated_at: 1_700_000_000_001,
                    last_recalled_at: None,
                    recall_count: 0,
                },
                sources,
                entities: vec!["MCP Vault".to_owned()],
                tags: vec!["migration".to_owned()],
                relations: Vec::new(),
            },
            None,
        )
        .await
        .unwrap();
}

fn legacy_source(
    context: &VaultContext,
    memory_id: MemoryId,
    source_type: &str,
    note: Option<(mcp_vault_domain::FileId, VaultPath, Revision)>,
) -> MemorySourceRecord {
    MemorySourceRecord {
        id: MemorySourceId::new(),
        vault_id: context.id(),
        memory_id,
        source_type: source_type.to_owned(),
        note_file_id: note.as_ref().map(|(id, _, _)| *id),
        note_path: note.as_ref().map(|(_, path, _)| path.clone()),
        note_revision: note.map(|(_, _, revision)| revision),
        heading_path: Vec::new(),
        start_line: None,
        end_line: None,
        excerpt_hash: None,
        actor_id: None,
        created_at: 1,
    }
}

#[tokio::test]
async fn explicit_memory_is_direct_idempotent_revisioned_and_physically_deleted() {
    let (directory, state, context, core, service) = fixture("explicit-v21").await;
    let input = RememberInput {
        content: "MCP Vault writes must use expected revisions.".to_owned(),
        memory_type: Some(MemoryType::Constraint),
        importance: None,
        confidence: Some(0.9),
        valid_from: Some(1_700_000_000_000),
        valid_to: Some(1_900_000_000_000),
        tags: vec!["writes".to_owned()],
        entities: vec!["MCP Vault".to_owned()],
        idempotency_key: Some("explicit-command-1".to_owned()),
        origin: MemoryOrigin::ExplicitAgent,
        extraction: json!({"caller_note": "preserved optional metadata"}),
        ..RememberInput::default()
    };
    let first = service
        .remember(&context, &core, input.clone())
        .await
        .unwrap();
    let first = first.memory.unwrap();
    assert_eq!(first.ownership, MemoryOwnership::Explicit);
    assert_eq!(first.importance, None);
    assert_eq!(first.confidence, Some(0.9));

    let repeated = service.remember(&context, &core, input).await.unwrap();
    assert_eq!(repeated.outcome, "stored_existing");
    assert_eq!(repeated.memory.unwrap().id, first.id);

    let (provider_id, model_id) = seed_vector_model(&state).await;
    seed_memory_vector(
        &state,
        &context,
        provider_id,
        model_id,
        first.id,
        "old-explicit-content",
    )
    .await;

    let updated = service
        .update(
            &context,
            &core,
            first.id,
            first.revision,
            MemoryUpdateInput {
                content: Some("MCP Vault canonical writes require expected revisions.".to_owned()),
                confidence: Some(None),
                ..MemoryUpdateInput::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.revision, Revision::new(2));
    assert_eq!(updated.confidence, None);
    assert!(
        state
            .providers()
            .list_embeddings(&context, model_id, "memory", 100, 0)
            .await
            .unwrap()
            .is_empty(),
        "content replacement must eagerly delete every old vector"
    );

    let canonical_path = updated.canonical_path.clone().unwrap();
    let mut canonical = core.read_managed(&context, &canonical_path).await.unwrap();
    let mut bytes = Vec::new();
    canonical.reader.read_to_end(&mut bytes).await.unwrap();
    let markdown = String::from_utf8(bytes).unwrap();
    assert!(markdown.contains("MCP Vault canonical writes require expected revisions."));
    assert!(!markdown.contains("status:"));
    assert!(!markdown.contains("supersed"));

    // Simulate a process exit after Vault Core committed the canonical bytes
    // but before the current projection transaction. The retry must adopt the
    // exact file instead of writing another revision or reporting conflict.
    let crash_content = "MCP Vault canonical writes require CAS revisions.";
    let advanced_markdown = markdown.replacen("revision: 2", "revision: 3", 1).replacen(
        "MCP Vault canonical writes require expected revisions.",
        crash_content,
        1,
    );
    let advanced_file = core
        .replace_managed_bytes(
            &context,
            &canonical_path,
            updated.canonical_revision.unwrap(),
            advanced_markdown.as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    let recovered = service
        .update(
            &context,
            &core,
            first.id,
            updated.revision,
            MemoryUpdateInput {
                content: Some(crash_content.to_owned()),
                ..MemoryUpdateInput::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(recovered.revision, Revision::new(3));
    assert_eq!(
        recovered.canonical_revision,
        Some(advanced_file.current_revision)
    );
    assert_eq!(recovered.content, crash_content);

    state
        .current_memory()
        .delete_explicit_projection(&context, first.id, recovered.revision)
        .await
        .unwrap();
    assert!(matches!(
        service.get(&context, first.id).await,
        Err(MemoryError::NotFound)
    ));
    let rebuild = service.rebuild(&context, &core).await.unwrap();
    assert_eq!(rebuild.projected, 1);
    assert_eq!(rebuild.quarantined, 0);
    let rebuilt = service.get(&context, first.id).await.unwrap();
    assert_eq!(rebuilt.id, first.id);
    assert_eq!(rebuilt.revision, recovered.revision);
    assert_eq!(rebuilt.content, crash_content);
    assert_eq!(rebuilt.memory_type, Some(MemoryType::Constraint));
    assert_eq!(rebuilt.importance, None);
    assert_eq!(rebuilt.confidence, None);
    assert_eq!(rebuilt.valid_from, Some(1_700_000_000_000));
    assert_eq!(rebuilt.valid_to, Some(1_900_000_000_000));
    assert_eq!(rebuilt.tags, vec!["writes"]);
    assert_eq!(rebuilt.entities, vec!["MCP Vault"]);

    seed_memory_vector(
        &state,
        &context,
        provider_id,
        model_id,
        first.id,
        "current-explicit-content",
    )
    .await;

    let deleted = service
        .forget(&context, &core, first.id, rebuilt.revision)
        .await
        .unwrap();
    assert!(deleted.deleted);
    assert!(!deleted.source_extraction_paused);
    assert!(matches!(
        service.get(&context, first.id).await,
        Err(MemoryError::NotFound)
    ));
    assert!(core.read_managed(&context, &canonical_path).await.is_err());
    assert!(
        state
            .providers()
            .list_embeddings(&context, model_id, "memory", 100, 0)
            .await
            .unwrap()
            .is_empty(),
        "physical deletion must remove all model/profile vector variants"
    );

    let restarted_core = VaultCore::new(
        state.clone(),
        directory.path().join("history"),
        Default::default(),
        StorageOptions::default(),
        Default::default(),
    );
    let restarted = MemoryService::new(
        state.clone(),
        AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[23_u8; 32]).unwrap(),
        ),
    );
    let after_restart = restarted.rebuild(&context, &restarted_core).await.unwrap();
    assert_eq!(after_restart.projected, 0);
    assert_eq!(after_restart.quarantined, 0);
    assert!(matches!(
        restarted.get(&context, first.id).await,
        Err(MemoryError::NotFound)
    ));
    assert!(
        restarted
            .recall(
                &context,
                RecallRequest {
                    query: "MCP Vault CAS revisions".to_owned(),
                    include_related_notes: false,
                    ..RecallRequest::default()
                },
            )
            .await
            .unwrap()
            .memories
            .is_empty(),
        "restart-style canonical rebuild must not resurrect a forgotten memory"
    );
}

#[tokio::test]
async fn note_source_owns_one_fail_closed_replaceable_set_and_move_needs_no_model() {
    let (_directory, state, context, core, service) = fixture("source-set-v21").await;
    let model_state = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, Arc::clone(&model_state)).await;
    let first_path = VaultPath::parse("notes/decision.md").unwrap();
    let created = core
        .create_bytes(
            &context,
            &first_path,
            b"# FIRST\nThe Alpha team requires Rust 1.94 only for backend builds. A switch to Go was proposed but not adopted.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let source_id = created.file.id;

    let first = service
        .extract_note(&context, &core, &first_path)
        .await
        .unwrap();
    assert_eq!(first.items_published, 2);
    assert_eq!(model_state.calls.load(Ordering::SeqCst), 1);
    let first_items = service
        .list(&context, Vec::new(), None, None, None, 20, 0)
        .await
        .unwrap();
    assert_eq!(first_items.len(), 2);
    assert!(
        first_items
            .iter()
            .any(|item| item.content.contains("not adopted"))
    );
    let extraction_model = state
        .providers()
        .list_models(None, 100)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    for item in &first_items {
        seed_memory_vector(
            &state,
            &context,
            extraction_model.provider_id,
            extraction_model.id,
            item.id,
            "old-source-content",
        )
        .await;
    }

    let moved_path = VaultPath::parse("decisions/backend.md").unwrap();
    let moved = core
        .move_entry(
            &context,
            &first_path,
            &moved_path,
            created.file.current_revision,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let reconciled = service
        .reconcile_current_source_event(&context, &core, source_id)
        .await
        .unwrap();
    assert_eq!(reconciled.moved, 1);
    assert_eq!(model_state.calls.load(Ordering::SeqCst), 1);
    let moved_items = service
        .list(&context, Vec::new(), None, None, None, 20, 0)
        .await
        .unwrap();
    assert!(moved_items.iter().all(|item| {
        item.sources
            .iter()
            .any(|source| source.path.as_ref() == Some(&moved_path))
    }));

    let changed = core
        .replace_bytes(
            &context,
            &moved_path,
            moved.file.current_revision,
            b"# SECOND\nThe Alpha team now requires Rust 1.95 for backend builds.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    assert!(
        service
            .list(&context, Vec::new(), None, None, None, 20, 0)
            .await
            .unwrap()
            .is_empty(),
        "a source hash change must immediately hide the old set"
    );
    let second = service
        .extract_note(&context, &core, &moved_path)
        .await
        .unwrap();
    assert_eq!(second.items_published, 1);
    assert_eq!(model_state.calls.load(Ordering::SeqCst), 2);
    let second_items = service
        .list(&context, Vec::new(), None, None, None, 20, 0)
        .await
        .unwrap();
    assert_eq!(second_items.len(), 1);
    assert!(second_items[0].content.contains("Rust 1.95"));
    assert!(
        state
            .providers()
            .list_embeddings(&context, extraction_model.id, "memory", 100, 0)
            .await
            .unwrap()
            .iter()
            .all(|embedding| !first_items
                .iter()
                .any(|item| embedding.object_id == item.id.to_string())),
        "full-set replacement must delete vectors for the replaced item IDs"
    );

    model_state.mode.store(MODEL_BLOCKED, Ordering::SeqCst);
    let in_flight = {
        let context = context.clone();
        let core = core.clone();
        let service = service.clone();
        let moved_path = moved_path.clone();
        tokio::spawn(async move {
            service
                .extract_note_with_options(
                    &context,
                    &core,
                    &moved_path,
                    NoteExtractionOptions {
                        include_evaluated: true,
                    },
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), model_state.started.notified())
        .await
        .expect("the stale extraction must reach the fake Provider before deletion");
    let deleted = service
        .forget(
            &context,
            &core,
            second_items[0].id,
            second_items[0].revision,
        )
        .await
        .unwrap();
    assert!(deleted.source_extraction_paused);
    model_state.release.notify_one();
    assert!(matches!(
        in_flight.await.unwrap(),
        Err(MemoryError::Conflict)
    ));
    model_state.mode.store(MODEL_NORMAL, Ordering::SeqCst);
    let paused_set = state
        .current_memory()
        .get_note_set_by_source(&context, source_id)
        .await
        .unwrap()
        .unwrap();
    assert!(paused_set.extraction_paused);

    let third_file = core
        .replace_bytes(
            &context,
            &moved_path,
            changed.file.current_revision,
            b"# THIRD\nThe Alpha team now requires Zig 0.15 for backend builds.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    let skipped = service
        .extract_note_with_options(
            &context,
            &core,
            &moved_path,
            NoteExtractionOptions {
                include_evaluated: true,
            },
        )
        .await
        .unwrap();
    assert!(skipped.already_evaluated);
    assert_eq!(model_state.calls.load(Ordering::SeqCst), 3);

    service
        .resume_note_extraction(
            &context,
            &core,
            source_id,
            paused_set.set_revision,
            Actor::system(),
        )
        .await
        .unwrap();
    let resumed = service
        .extract_note_with_options(
            &context,
            &core,
            &moved_path,
            NoteExtractionOptions {
                include_evaluated: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(resumed.items_published, 1);
    assert_eq!(model_state.calls.load(Ordering::SeqCst), 4);

    let current_set = state
        .current_memory()
        .get_note_set_by_source(&context, source_id)
        .await
        .unwrap()
        .unwrap();
    let current_items = service
        .list(&context, Vec::new(), None, None, None, 20, 0)
        .await
        .unwrap();
    state
        .current_memory()
        .delete_note_set_projection(&context, source_id, current_set.set_revision)
        .await
        .unwrap();
    assert!(
        service
            .list(&context, Vec::new(), None, None, None, 20, 0)
            .await
            .unwrap()
            .is_empty()
    );
    let rebuild = service.rebuild(&context, &core).await.unwrap();
    assert_eq!(rebuild.projected, 1);
    assert_eq!(rebuild.quarantined, 0);
    let rebuilt_items = service
        .list(&context, Vec::new(), None, None, None, 20, 0)
        .await
        .unwrap();
    assert_eq!(
        rebuilt_items.iter().map(|item| item.id).collect::<Vec<_>>(),
        current_items.iter().map(|item| item.id).collect::<Vec<_>>()
    );
    assert_eq!(rebuilt_items[0].content, current_items[0].content);
    seed_memory_vector(
        &state,
        &context,
        extraction_model.provider_id,
        extraction_model.id,
        rebuilt_items[0].id,
        "current-source-content",
    )
    .await;
    core.delete(
        &context,
        &moved_path,
        third_file.current_revision,
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let deleted_source = service
        .reconcile_current_source_event(&context, &core, source_id)
        .await
        .unwrap();
    assert_eq!(deleted_source.deleted, 1);
    assert_eq!(deleted_source.memories_removed, 1);
    assert!(
        state
            .current_memory()
            .get_note_set_by_source(&context, source_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        core.read_managed(&context, &current_set.canonical_path)
            .await
            .is_err()
    );
    assert!(matches!(
        service.get(&context, rebuilt_items[0].id).await,
        Err(MemoryError::NotFound)
    ));
    assert!(
        state
            .providers()
            .list_embeddings(&context, extraction_model.id, "memory", 100, 0)
            .await
            .unwrap()
            .is_empty(),
        "source deletion must remove current memory vectors"
    );

    let recreated = core
        .create_bytes(
            &context,
            &moved_path,
            b"# THIRD\nThe Alpha team now requires Zig 0.15 for backend builds.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    let recreated_result = service
        .extract_note(&context, &core, &moved_path)
        .await
        .unwrap();
    assert_eq!(recreated_result.items_published, 1);
    let recreated_set = state
        .current_memory()
        .get_note_set_by_source(&context, recreated.id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(recreated_set.id, current_set.id);
    let recreated_items = service
        .list(&context, Vec::new(), None, None, None, 20, 0)
        .await
        .unwrap();
    assert_eq!(recreated_items.len(), 1);
    assert!(recreated_items[0].content.contains("Zig 0.15"));
    assert!(
        first_items
            .iter()
            .chain(current_items.iter())
            .all(|old| old.id != recreated_items[0].id),
        "delete/recreate must allocate a fresh set and item identities even when Vault Core restores the path tombstone"
    );
}

#[tokio::test]
async fn duplicate_source_facts_and_explicit_memory_keep_independent_ownership() {
    let (_directory, state, context, core, service) = fixture("independent-owners-v21").await;
    let model_state = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, model_state).await;
    let first_path = VaultPath::parse("notes/owner-a.md").unwrap();
    let second_path = VaultPath::parse("notes/owner-b.md").unwrap();
    let first_file = core
        .create_bytes(
            &context,
            &first_path,
            b"# FIRST\nThe Alpha team requires Rust 1.94 only for backend builds.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    let second_file = core
        .create_bytes(
            &context,
            &second_path,
            b"# FIRST\nA second source independently confirms the Alpha team requirement.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    service
        .extract_note(&context, &core, &first_path)
        .await
        .unwrap();
    service
        .extract_note(&context, &core, &second_path)
        .await
        .unwrap();
    let first_set = state
        .current_memory()
        .get_note_set_by_source(&context, first_file.id)
        .await
        .unwrap()
        .unwrap();
    let second_set = state
        .current_memory()
        .get_note_set_by_source(&context, second_file.id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first_set.id, second_set.id);
    let first_items = state
        .current_memory()
        .list_note_set_items(&context, first_set.id)
        .await
        .unwrap();
    let second_items = state
        .current_memory()
        .list_note_set_items(&context, second_set.id)
        .await
        .unwrap();
    assert!(first_items.iter().all(|first| {
        second_items
            .iter()
            .all(|second| first.memory.id != second.memory.id)
    }));

    let duplicate_content = first_items[0].memory.content.clone();
    let explicit = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: duplicate_content.clone(),
                idempotency_key: Some("independent-explicit-owner".to_owned()),
                ..RememberInput::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    let recall = service
        .recall(
            &context,
            RecallRequest {
                query: "Alpha Rust 1.94 backend requirement".to_owned(),
                include_related_notes: false,
                ..RecallRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        recall
            .memories
            .iter()
            .filter(|memory| memory.content == duplicate_content)
            .count(),
        1,
        "recall may deduplicate presentation without merging durable owners"
    );

    core.delete(
        &context,
        &first_path,
        first_file.current_revision,
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    service
        .reconcile_current_source_event(&context, &core, first_file.id)
        .await
        .unwrap();
    for item in &first_items {
        assert!(matches!(
            service.get(&context, item.memory.id).await,
            Err(MemoryError::NotFound)
        ));
    }
    for item in &second_items {
        assert!(service.get(&context, item.memory.id).await.is_ok());
    }
    assert!(service.get(&context, explicit.id).await.is_ok());

    core.delete(
        &context,
        &second_path,
        second_file.current_revision,
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    service
        .reconcile_current_source_event(&context, &core, second_file.id)
        .await
        .unwrap();
    for item in &second_items {
        assert!(matches!(
            service.get(&context, item.memory.id).await,
            Err(MemoryError::NotFound)
        ));
    }
    assert!(service.get(&context, explicit.id).await.is_ok());
}

#[tokio::test]
async fn extraction_distinguishes_empty_failure_and_inflight_source_change() {
    let (_directory, state, context, core, service) = fixture("extraction-boundaries-v21").await;
    let model_state = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, Arc::clone(&model_state)).await;
    let path = VaultPath::parse("notes/extraction-boundaries.md").unwrap();
    let created = core
        .create_bytes(
            &context,
            &path,
            b"# FIRST\nThe Alpha team requires Rust 1.94 only for backend builds.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    service.extract_note(&context, &core, &path).await.unwrap();
    let original_ids = service
        .list(&context, Vec::new(), None, None, None, 20, 0)
        .await
        .unwrap()
        .into_iter()
        .map(|memory| memory.id)
        .collect::<Vec<_>>();
    assert!(!original_ids.is_empty());

    model_state.mode.store(MODEL_INVALID_ROOT, Ordering::SeqCst);
    assert!(
        service
            .extract_note_with_options(
                &context,
                &core,
                &path,
                NoteExtractionOptions {
                    include_evaluated: true,
                },
            )
            .await
            .is_err()
    );
    assert_eq!(
        service
            .list(&context, Vec::new(), None, None, None, 20, 0)
            .await
            .unwrap()
            .iter()
            .map(|memory| memory.id)
            .collect::<Vec<_>>(),
        original_ids,
        "an invalid response for an unchanged source must retain its current complete set"
    );

    let changed = core
        .replace_bytes(
            &context,
            &path,
            created.current_revision,
            b"# SECOND\nThe Alpha team now requires Rust 1.95 for backend builds.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    assert!(
        service
            .list(&context, Vec::new(), None, None, None, 20, 0)
            .await
            .unwrap()
            .is_empty(),
        "a changed source must fail closed before regeneration"
    );
    assert!(service.extract_note(&context, &core, &path).await.is_err());
    assert!(
        service
            .list(&context, Vec::new(), None, None, None, 20, 0)
            .await
            .unwrap()
            .is_empty(),
        "invalid output must never be interpreted as an empty successful set"
    );

    model_state.mode.store(MODEL_EMPTY_SET, Ordering::SeqCst);
    let empty = service.extract_note(&context, &core, &path).await.unwrap();
    assert!(empty.empty_set_published);
    assert_eq!(empty.items_published, 0);
    let empty_set = state
        .current_memory()
        .get_note_set_by_source(&context, changed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        empty_set.source_content_hash,
        changed.content_hash.clone().unwrap()
    );

    let first_inflight = core
        .replace_bytes(
            &context,
            &path,
            changed.current_revision,
            b"# BLOCK\nFirst in-flight source content.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    model_state.mode.store(MODEL_BLOCKED, Ordering::SeqCst);
    let extraction = {
        let context = context.clone();
        let core = core.clone();
        let service = service.clone();
        let path = path.clone();
        tokio::spawn(async move { service.extract_note(&context, &core, &path).await })
    };
    tokio::time::timeout(Duration::from_secs(5), model_state.started.notified())
        .await
        .expect("the fake Provider should receive the in-flight source");
    core.replace_bytes(
        &context,
        &path,
        first_inflight.current_revision,
        b"# BLOCK\nSecond source content wins the race.",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    model_state.release.notify_one();
    assert!(matches!(
        extraction.await.unwrap(),
        Err(MemoryError::Conflict)
    ));
    assert!(
        service
            .list(&context, Vec::new(), None, None, None, 20, 0)
            .await
            .unwrap()
            .is_empty(),
        "the stale in-flight result must not publish after the source hash changes"
    );
    assert!(
        state
            .current_memory()
            .prepared_note_set_snapshot(&context, first_inflight.id)
            .await
            .unwrap()
            .is_none()
    );

    model_state.mode.store(MODEL_NORMAL, Ordering::SeqCst);
}

#[tokio::test]
async fn prepared_snapshot_recovers_after_canonical_commit_without_another_model_call() {
    let (_directory, state, context, core, service) = fixture("snapshot-recovery-v21").await;
    let model_state = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, Arc::clone(&model_state)).await;
    let path = VaultPath::parse("notes/snapshot-recovery.md").unwrap();
    let source = core
        .create_bytes(
            &context,
            &path,
            b"# FIRST\nThe Alpha team requires Rust 1.94 only for backend builds.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    let failing_core = core
        .clone()
        .with_failure_injector(Arc::new(FailOnceAt::new(CommitPhase::MetadataCommitted)));

    assert!(
        service
            .extract_note(&context, &failing_core, &path)
            .await
            .is_err(),
        "the injected post-canonical fault must interrupt projection publication"
    );
    assert_eq!(model_state.calls.load(Ordering::SeqCst), 1);
    assert!(
        service
            .list(&context, Vec::new(), None, None, None, 20, 0)
            .await
            .unwrap()
            .is_empty(),
        "a committed canonical file is not current memory until its exact snapshot publishes"
    );
    let prepared = state
        .current_memory()
        .prepared_note_set_snapshot(&context, source.id)
        .await
        .unwrap()
        .expect("the generated snapshot must remain retryable");
    let prepared_ids = prepared.items.as_array().unwrap().iter().map(|item| {
        MemoryId::parse(item["id"].as_str().expect("prepared ID must serialize")).unwrap()
    });
    let expected_ids = prepared_ids.collect::<Vec<_>>();
    assert!(!expected_ids.is_empty());
    assert!(
        core.read_managed(&context, &prepared.canonical_path)
            .await
            .is_ok(),
        "the fault is after Vault Core committed the canonical bytes"
    );

    let retried = service.extract_note(&context, &core, &path).await.unwrap();
    assert!(retried.reused_prepared_snapshot);
    assert_eq!(model_state.calls.load(Ordering::SeqCst), 1);
    let published_ids = service
        .list(&context, Vec::new(), None, None, None, 20, 0)
        .await
        .unwrap()
        .into_iter()
        .map(|memory| memory.id)
        .collect::<Vec<_>>();
    assert_eq!(published_ids, expected_ids);
    assert!(
        state
            .current_memory()
            .prepared_note_set_snapshot(&context, source.id)
            .await
            .unwrap()
            .is_none(),
        "the successfully applied snapshot must no longer be retryable"
    );
}

#[tokio::test]
async fn legacy_migration_is_preflighted_non_destructive_and_preserves_only_safe_explicit_rows() {
    let (_directory, state, context, core, service) = fixture("migration-v21").await;
    let source_path = VaultPath::parse("notes/legacy-source.md").unwrap();
    let source = core
        .create_bytes(
            &context,
            &source_path,
            b"# Legacy source\nCurrent source truth is regenerated under v2.1.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    let source_identity = (source.id, source_path.clone(), source.current_revision);

    let safe_id = MemoryId::new();
    seed_legacy_memory(
        &state,
        &context,
        safe_id,
        "active",
        "Explicit legacy decision must be preserved.",
        vec![legacy_source(&context, safe_id, "explicit_agent", None)],
    )
    .await;
    let note_id = MemoryId::new();
    seed_legacy_memory(
        &state,
        &context,
        note_id,
        "active",
        "Derived legacy summary must be regenerated from its source.",
        vec![legacy_source(
            &context,
            note_id,
            "note",
            Some(source_identity.clone()),
        )],
    )
    .await;
    let mixed_id = MemoryId::new();
    seed_legacy_memory(
        &state,
        &context,
        mixed_id,
        "active",
        "Mixed ownership requires an operator decision.",
        vec![
            legacy_source(&context, mixed_id, "explicit_admin", None),
            legacy_source(&context, mixed_id, "note", Some(source_identity.clone())),
        ],
    )
    .await;
    let unsupported_id = MemoryId::new();
    seed_legacy_memory(
        &state,
        &context,
        unsupported_id,
        "active",
        "Unsupported provenance remains report-only.",
        Vec::new(),
    )
    .await;
    let historical_id = MemoryId::new();
    seed_legacy_memory(
        &state,
        &context,
        historical_id,
        "archived",
        "Historical content remains only in legacy backup scope.",
        vec![legacy_source(
            &context,
            historical_id,
            "explicit_agent",
            None,
        )],
    )
    .await;

    let preflight = state
        .current_memory()
        .migration_preflight(&context)
        .await
        .unwrap();
    assert_eq!(preflight.legacy_total, 5);
    assert_eq!(preflight.safe_explicit, 1);
    assert_eq!(preflight.note_derived, 1);
    assert_eq!(preflight.mixed_source, 1);
    assert_eq!(preflight.unsupported, 1);
    assert_eq!(preflight.historical, 1);
    assert!(preflight.classified_state_hash.starts_with("sha256:"));
    let reviewed_preflight_hash = preflight.fingerprint().unwrap();
    assert!(reviewed_preflight_hash.starts_with("sha256:"));
    assert_eq!(
        state
            .current_memory()
            .migration_preflight(&context)
            .await
            .unwrap()
            .fingerprint()
            .unwrap(),
        reviewed_preflight_hash
    );
    assert!(
        service
            .list(&context, Vec::new(), None, None, None, 20, 0)
            .await
            .unwrap()
            .is_empty(),
        "preflight must not publish or delete knowledge"
    );

    let mut changed_after_review = state
        .memory()
        .get_bundle(&context, safe_id)
        .await
        .unwrap()
        .unwrap();
    changed_after_review.memory.extraction = json!({"legacy": true, "reviewed_after_preflight": 1});
    state
        .memory()
        .replace_bundle(
            &context,
            &changed_after_review,
            Some(changed_after_review.memory.revision),
        )
        .await
        .unwrap();
    assert!(matches!(
        service
            .migrate_legacy_v2_1(&context, &core, &reviewed_preflight_hash, Actor::system(),)
            .await,
        Err(MemoryError::Conflict)
    ));
    assert!(
        service
            .list(&context, Vec::new(), None, None, None, 20, 0)
            .await
            .unwrap()
            .is_empty(),
        "a stale confirmation must not partially migrate unchanged ownership counts"
    );
    let confirmed_preflight_hash = state
        .current_memory()
        .migration_preflight(&context)
        .await
        .unwrap()
        .fingerprint()
        .unwrap();
    assert_ne!(confirmed_preflight_hash, reviewed_preflight_hash);

    let migrated = service
        .migrate_legacy_v2_1(&context, &core, &confirmed_preflight_hash, Actor::system())
        .await
        .unwrap();
    assert_eq!(migrated.migrated_explicit, 1);
    assert_eq!(migrated.safe_explicit, 1);
    assert_eq!(migrated.note_derived, 1);
    assert!(!migrated.legacy_rows_deleted);
    assert!(!migrated.completed);
    assert!(migrated.unresolved_ids.contains(&mixed_id.to_string()));
    assert!(
        migrated
            .unresolved_ids
            .contains(&unsupported_id.to_string())
    );

    let current = service.get(&context, safe_id).await.unwrap();
    assert_eq!(current.id, safe_id);
    assert_eq!(
        current.content,
        "Explicit legacy decision must be preserved."
    );
    assert_eq!(current.importance, Some(0.83));
    assert_eq!(current.confidence, Some(0.77));
    assert_eq!(current.valid_from, Some(1_700_000_000_000));
    assert_eq!(current.valid_to, Some(1_900_000_000_000));
    assert_eq!(current.tags, vec!["migration"]);
    assert_eq!(current.entities, vec!["MCP Vault"]);
    assert!(matches!(
        service.get(&context, note_id).await,
        Err(MemoryError::NotFound)
    ));
    assert!(matches!(
        service.get(&context, mixed_id).await,
        Err(MemoryError::NotFound)
    ));
    let projection = state
        .current_memory()
        .get(&context, safe_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        projection.memory.metadata["migration"]["numeric_metadata_provenance"],
        "legacy_unknown"
    );
    assert!(
        state
            .memory()
            .get_memory(&context, safe_id)
            .await
            .unwrap()
            .is_some(),
        "authorized migration keeps legacy rows for backup/report recovery"
    );
    assert!(
        core.read_managed(&context, current.canonical_path.as_ref().unwrap())
            .await
            .is_ok()
    );

    let repeated = service
        .migrate_legacy_v2_1(&context, &core, &confirmed_preflight_hash, Actor::system())
        .await
        .unwrap();
    assert_eq!(repeated.migrated_explicit, 0);
    assert_eq!(repeated.already_current, 1);
}

#[tokio::test]
async fn recall_gates_unrelated_queries_and_never_exposes_another_vault() {
    let (directory, state, first, first_core, service) = fixture("recall-v21").await;
    let second = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("recall-v21-other").unwrap(),
        directory.path().join("other-vault"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&second, "other", VaultStatus::Active)
        .await
        .unwrap();
    let other_core = VaultCore::new(
        state.clone(),
        directory.path().join("other-history"),
        Default::default(),
        StorageOptions::default(),
        Default::default(),
    );
    let first_memory = service
        .remember(
            &first,
            &first_core,
            RememberInput {
                content: "Production WebDAV writes require If-Match revisions.".to_owned(),
                idempotency_key: Some("first-recall-memory".to_owned()),
                ..RememberInput::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    service
        .remember(
            &second,
            &other_core,
            RememberInput {
                content: "Secret lunar orchard launch phrase.".to_owned(),
                idempotency_key: Some("second-recall-memory".to_owned()),
                ..RememberInput::default()
            },
        )
        .await
        .unwrap();

    let relevant = service
        .recall(
            &first,
            RecallRequest {
                query: "What revision precondition do WebDAV writes require?".to_owned(),
                include_related_notes: false,
                ..RecallRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        relevant.memories.first().map(|item| item.id),
        Some(first_memory.id)
    );

    let unrelated = service
        .recall(
            &first,
            RecallRequest {
                query: "lunar orchard phrase".to_owned(),
                include_related_notes: false,
                ..RecallRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(unrelated.memories.is_empty());

    let long = service
        .remember(
            &first,
            &first_core,
            RememberInput {
                content: format!("priority budget marker {}", "long-condition ".repeat(200)),
                idempotency_key: Some("long-budget-memory".to_owned()),
                ..RememberInput::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    let short = service
        .remember(
            &first,
            &first_core,
            RememberInput {
                content: "budget marker fits.".to_owned(),
                idempotency_key: Some("short-budget-memory".to_owned()),
                ..RememberInput::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    let budgeted = service
        .recall(
            &first,
            RecallRequest {
                query: "budget marker".to_owned(),
                context: RecallContext {
                    active_project: Some("priority".to_owned()),
                    ..RecallContext::default()
                },
                include_related_notes: false,
                max_tokens: 320,
                ..RecallRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(serde_json::to_vec(&budgeted).unwrap().len() <= 320 * 4);
    assert_eq!(budgeted.candidate_memory_count, 2);
    assert_eq!(budgeted.relevant_memory_count, 2);
    assert!(budgeted.truncated);
    assert!(budgeted.memories.iter().all(|memory| memory.id != long.id));
    assert_eq!(
        budgeted.memories.first().map(|memory| memory.id),
        Some(short.id)
    );

    let reclaimable = service
        .remember(
            &first,
            &first_core,
            RememberInput {
                content: format!("shared reclaim marker {}", "bounded-detail ".repeat(15)),
                idempotency_key: Some("shared-budget-memory".to_owned()),
                ..RememberInput::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    let shared_budget = service
        .recall(
            &first,
            RecallRequest {
                query: "shared reclaim marker".to_owned(),
                include_related_notes: true,
                max_related_notes: 5,
                max_tokens: 500,
                ..RecallRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        shared_budget.memories.first().map(|memory| memory.id),
        Some(reclaimable.id),
        "unused related-note reservation must return to the shared response budget"
    );
}

#[tokio::test]
async fn derived_forget_works_after_rebuild_without_original_provider() {
    let (_directory, state, context, core, service) = fixture("local-delete").await;
    let model_state = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, model_state.clone()).await;
    let path = VaultPath::parse("source.md").unwrap();
    let source = core
        .create_bytes(
            &context,
            &path,
            b"# FIRST\nAlpha backend decisions.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    service.extract_note(&context, &core, &path).await.unwrap();
    let mut set = state
        .current_memory()
        .get_note_set_by_source(&context, source.id)
        .await
        .unwrap()
        .unwrap();
    let mut bundles = state
        .current_memory()
        .list_note_set_items(&context, set.id)
        .await
        .unwrap();
    // This is the supported projection shape after restoring canonical Markdown
    // into an installation where its original model no longer exists.
    set.provider_id = None;
    set.model_id = None;
    for bundle in &mut bundles {
        bundle.note_set = Some(set.clone());
    }
    state
        .current_memory()
        .restore_note_set_projection(&context, &set, &bundles)
        .await
        .unwrap();
    let calls = model_state.calls.load(Ordering::SeqCst);
    let deleted = service
        .forget(
            &context,
            &core,
            bundles[0].memory.id,
            bundles[0].memory.revision,
        )
        .await
        .unwrap();
    assert!(deleted.source_extraction_paused);
    assert!(matches!(
        service.get(&context, deleted.id).await,
        Err(MemoryError::NotFound)
    ));
    let remaining = service.get(&context, bundles[1].memory.id).await.unwrap();
    assert_eq!(remaining.content, bundles[1].memory.content);
    assert_eq!(remaining.revision, bundles[1].memory.revision);
    service
        .forget(&context, &core, remaining.id, remaining.revision)
        .await
        .unwrap();
    let empty = state
        .current_memory()
        .get_note_set_by_source(&context, source.id)
        .await
        .unwrap()
        .unwrap();
    assert!(empty.extraction_paused);
    assert!(
        state
            .current_memory()
            .list_note_set_items(&context, empty.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(model_state.calls.load(Ordering::SeqCst), calls);
}

async fn configure_test_embeddings(
    state: &StateStore,
    context: &VaultContext,
) -> (ProviderService, ModelId) {
    configure_test_embeddings_with_gate(state, context, Arc::new(CurrentSetModelState::default()))
        .await
}

async fn configure_test_embeddings_with_gate(
    state: &StateStore,
    context: &VaultContext,
    gate: Arc<CurrentSetModelState>,
) -> (ProviderService, ModelId) {
    async fn embeddings(
        AxumState(gate): AxumState<Arc<CurrentSetModelState>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        if gate.mode.load(Ordering::SeqCst) == MODEL_BLOCKED
            && body["input"]
                .as_array()
                .is_some_and(|inputs| inputs.len() == 1)
            && gate.calls.fetch_add(1, Ordering::SeqCst) == 1
        {
            gate.started.notify_one();
            gate.release.notified().await;
        }
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/memory-quality/calibration.json"
        ))
        .unwrap();
        let documents = corpus["documents"].as_array().unwrap();
        let queries = corpus["queries"].as_array().unwrap();
        Json(
            json!({"data": body["input"].as_array().unwrap().iter().enumerate().map(|(index,input)| {
            let text=input.as_str().unwrap();
            let doc=documents.iter().position(|doc|text.to_lowercase().contains(&doc["content"].as_str().unwrap().to_lowercase()));
            let query=queries.iter().find(|query|query["query"].as_str()==Some(text));
            let coordinate=doc.or_else(||query.and_then(|query|query["relevant"][0].as_str()).and_then(|id|documents.iter().position(|doc|doc["id"].as_str()==Some(id))))
                .unwrap_or(if query.is_some(){63}else{60});
            let mut vector=vec![0.0_f32;64]; vector[coordinate]=1.0;
            json!({"index":index,"embedding":vector})
        }).collect::<Vec<_>>()}),
        )
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/v1/embeddings", post(embeddings))
                .with_state(gate),
        )
        .await
        .unwrap();
    });
    let providers = ProviderService::new(
        state.clone(),
        AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[23_u8; 32]).unwrap(),
        ),
    );
    providers
        .set_provider_mode(context, ProviderMode::LocalOnly, None)
        .await
        .unwrap();
    let provider = providers
        .create_provider(ProviderInput {
            name: "test-embedding".into(),
            kind: ProviderKind::EmbeddingHttp,
            base_url: url::Url::parse(&format!("http://{address}/v1/")).unwrap(),
            settings: ProviderSettings::default(),
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();
    let model = providers
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "contract-embedding".into(),
            capabilities: ModelCapabilities {
                embeddings: true,
                dimension: Some(64),
                ..Default::default()
            },
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await
        .unwrap();
    providers
        .bind_model(Some(context), "embedding_memory", model.id, json!({}), None)
        .await
        .unwrap();
    (providers, model.id)
}

#[tokio::test]
async fn semantic_recall_respects_type_validity_and_importance() {
    let (_dir, state, context, core, service) = fixture("semantic-filter").await;
    let (_providers, model_id) = configure_test_embeddings(&state, &context).await;
    let mut expected = None;
    for (index, (kind, from, to, importance)) in [
        (MemoryType::Fact, Some(100), Some(200), 0.8),
        (MemoryType::Preference, None, None, 0.8),
        (MemoryType::Fact, Some(101), None, 0.8),
        (MemoryType::Fact, None, Some(100), 0.8),
        (MemoryType::Fact, None, None, 0.1),
    ]
    .into_iter()
    .enumerate()
    {
        let memory = service
            .remember(
                &context,
                &core,
                RememberInput {
                    content: format!("Synthetic assertion number {index} about an orchard."),
                    memory_type: Some(kind),
                    valid_from: from,
                    valid_to: to,
                    importance: Some(importance),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .memory
            .unwrap();
        if index == 0 {
            expected = Some(memory.id);
        }
        let bundle = state
            .current_memory()
            .get(&context, memory.id)
            .await
            .unwrap()
            .unwrap();
        service
            .reembed_sources(
                &context,
                model_id,
                &[mcp_vault_providers::EmbeddingSourceRef {
                    object_type: "memory".into(),
                    object_id: memory.id.to_string(),
                    chunk_key: "body-v3:0000".into(),
                    content_hash: bundle.memory.content_hash,
                }],
            )
            .await
            .unwrap();
    }
    let profile = service
        .calibration_profile(&context, "memory")
        .await
        .unwrap()
        .unwrap();
    let report = service
        .execute_retrieval_calibration(&context, &profile)
        .await
        .unwrap();
    assert!(
        report.passed,
        "contract embeddings must pass the actual calculation: {}",
        serde_json::to_string(&report).unwrap()
    );
    let result = service
        .recall(
            &context,
            RecallRequest {
                query: "完全不同的语义查询".into(),
                types: vec![MemoryType::Fact],
                valid_at: Some(100),
                min_importance: 0.5,
                include_related_notes: false,
                include_score_breakdown: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        result
            .memories
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        vec![expected.unwrap()]
    );
    assert!(
        result.memories[0]
            .score_breakdown
            .as_ref()
            .unwrap()
            .contains_key("semantic_cosine")
    );
}

#[tokio::test]
async fn context_only_relevant_candidate_survives_admission() {
    let (_dir, state, context, core, service) = fixture("context-window").await;
    for index in 0..51 {
        service
            .remember(
                &context,
                &core,
                RememberInput {
                    content: format!("orchard schedule orchard schedule {index}"),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    let target = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: format!("orchard schedule {}", "background details ".repeat(80)),
                entities: vec!["target-project".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    let unrelated = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: "unrelated telescope installation".into(),
                entities: vec!["target-project".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    let fts = state
        .current_memory()
        .search_fts(
            &context,
            "\"orchard\" OR \"schedule\"",
            &Default::default(),
            50,
        )
        .await
        .unwrap();
    assert!(
        !fts.iter().any(|hit| hit.memory.id == target.id),
        "fixture must reach outside FTS window"
    );
    let result = service
        .recall(
            &context,
            RecallRequest {
                query: "orchard schedule".into(),
                context: RecallContext {
                    entities: vec!["target-project".into()],
                    ..Default::default()
                },
                max_results: 100,
                max_tokens: 32000,
                include_related_notes: false,
                include_score_breakdown: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let hit = result
        .memories
        .iter()
        .find(|hit| hit.id == target.id)
        .expect("relevant context candidate should survive despite a small ranking contribution");
    assert!(
        !hit.score_breakdown
            .as_ref()
            .unwrap()
            .contains_key("lexical_rrf")
    );
    assert!(!result.memories.iter().any(|hit| hit.id == unrelated.id));
}

#[tokio::test]
async fn semantic_memory_rank_is_invariant_to_duplicate_chunks() {
    let (_dir, state, context, core, service) = fixture("semantic-object-rank").await;
    let (_providers, model_id) = configure_test_embeddings(&state, &context).await;
    let mut ids = Vec::new();
    for content in ["orchard ".repeat(8000), "Synthetic second object".into()] {
        let memory = service
            .remember(
                &context,
                &core,
                RememberInput {
                    content,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .memory
            .unwrap();
        let bundle = state
            .current_memory()
            .get(&context, memory.id)
            .await
            .unwrap()
            .unwrap();
        let sources = (0..32)
            .map(|ordinal| mcp_vault_providers::EmbeddingSourceRef {
                object_type: "memory".into(),
                object_id: memory.id.to_string(),
                chunk_key: format!("body-v3:{ordinal:04}"),
                content_hash: bundle.memory.content_hash.clone(),
            })
            .collect::<Vec<_>>();
        service
            .reembed_sources(&context, model_id, &sources)
            .await
            .unwrap();
        ids.push(memory.id);
    }
    let records = state
        .providers()
        .list_embeddings(&context, model_id, "memory", 100, 0)
        .await
        .unwrap();
    let duplicates = records
        .iter()
        .filter(|row| row.object_id == ids[0].to_string() && row.chunk_key != "body-v3:0000")
        .cloned()
        .collect::<Vec<_>>();
    assert!(duplicates.len() >= 30);
    let mut a = vec![0.0_f32; 64];
    a[60] = 1.0;
    let mut b = vec![0.0_f32; 64];
    b[60] = 0.9;
    b[61] = 0.19_f32.sqrt();
    for row in &records {
        state
            .providers()
            .upsert_embedding(
                &context,
                row,
                if row.object_id == ids[0].to_string() {
                    &a
                } else {
                    &b
                },
            )
            .await
            .unwrap();
    }
    for row in &duplicates {
        state
            .providers()
            .delete_embedding(&context, row.id)
            .await
            .unwrap();
    }
    let profile = service
        .calibration_profile(&context, "memory")
        .await
        .unwrap()
        .unwrap();
    assert!(
        service
            .execute_retrieval_calibration(&context, &profile)
            .await
            .unwrap()
            .passed
    );
    let request = RecallRequest {
        query: "完全无字面重合的查询".into(),
        include_related_notes: false,
        include_score_breakdown: true,
        ..Default::default()
    };
    let before = service.recall(&context, request.clone()).await.unwrap();
    for row in &duplicates {
        state
            .providers()
            .upsert_embedding(&context, row, &a)
            .await
            .unwrap();
    }
    let after = service.recall(&context, request).await.unwrap();
    let score = |result: &mcp_vault_memory::RecallResult| {
        result
            .memories
            .iter()
            .find(|memory| memory.id == ids[1])
            .unwrap()
            .score_breakdown
            .clone()
            .unwrap()
    };
    assert_eq!(score(&before)["semantic_object_rank"], 2.0);
    assert_eq!(
        score(&before)["semantic_rrf"],
        score(&after)["semantic_rrf"]
    );
    assert_eq!(score(&after)["semantic_object_rank"], 2.0);
}

#[tokio::test]
async fn recall_budget_counts_actual_heading_text_and_metadata() {
    let (_dir, state, context, core, service) = fixture("heading-budget").await;
    let long_path = VaultPath::parse("long-heading-source.md").unwrap();
    let mut content = String::from("# orchard schedule\norchard schedule\n");
    for i in 0..60 {
        content.push_str(&format!(
            "\n## heading-{i} {}\norchard schedule background\n",
            "x".repeat(100)
        ));
    }
    for (path, body) in [
        (&long_path, content.as_str()),
        (
            &VaultPath::parse("short.md").unwrap(),
            "# orchard schedule\norchard schedule concise assertion",
        ),
    ] {
        core.create_bytes(
            &context,
            path,
            body.as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    }
    mcp_vault_indexer::IndexService::new(state.clone())
        .rebuild_vault(&core, &context)
        .await
        .unwrap();
    let result = service
        .recall(
            &context,
            RecallRequest {
                query: "orchard schedule".into(),
                max_tokens: 600,
                include_score_breakdown: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(serde_json::to_vec(&result).unwrap().len() <= 600 * 4);
    assert_eq!(result.related_notes.len(), 1);
    assert_eq!(result.related_notes[0].path.as_str(), "short.md");
    assert!(result.truncated);
    let no_answer = service
        .recall(
            &context,
            RecallRequest {
                query: "orbital telescope launch".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(no_answer.memories.is_empty());
    assert!(no_answer.related_notes.is_empty());
}

#[tokio::test]
async fn calibration_controls_cancel_retry_pause_and_signature_changes_are_real() {
    let (_dir, state, context, _core, service) = fixture("calibration-controls").await;
    let (providers, _model) = configure_test_embeddings(&state, &context).await;
    let job = service
        .request_retrieval_calibration(&context, "memory")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        service
            .request_retrieval_calibration(&context, "memory")
            .await
            .unwrap(),
        Some(job)
    );
    service
        .cancel_retrieval_calibration(&context, job)
        .await
        .unwrap();
    let status = service
        .calibration_status(&context, "memory")
        .await
        .unwrap();
    assert_eq!(status.run.unwrap().status, "cancelled");
    assert!(
        service
            .ensure_retrieval_calibration(&context)
            .await
            .unwrap()
            .is_none()
    );
    let retry = service
        .request_retrieval_calibration(&context, "memory")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(retry, job);
    let profile = service
        .calibration_profile(&context, "memory")
        .await
        .unwrap()
        .unwrap();
    let report = service
        .execute_retrieval_calibration(&context, &profile)
        .await
        .unwrap();
    assert!(report.passed);
    // Simulate a crash after saving the passed report but before publishing it.
    state
        .settings()
        .set_vault(
            &context,
            &format!("retrieval.calibration.active.memory.{}", profile.signature),
            &Value::Null,
            mcp_vault_domain::WritePrecondition::Unconditional,
            None,
        )
        .await
        .unwrap();
    assert!(
        !service
            .calibration_status(&context, "memory")
            .await
            .unwrap()
            .active
    );
    let repeated = service
        .execute_retrieval_calibration(&context, &profile)
        .await
        .unwrap();
    assert_eq!(
        repeated.requests, report.requests,
        "completed report must not cause network replay"
    );
    assert!(
        service
            .calibration_status(&context, "memory")
            .await
            .unwrap()
            .active
    );
    providers
        .bind_model(
            Some(&context),
            "embedding_note",
            profile.model_id,
            json!({}),
            None,
        )
        .await
        .unwrap();
    let note_profile = service
        .calibration_profile(&context, "note")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(profile.signature, note_profile.signature);
    let note_report = service
        .execute_retrieval_calibration(&context, &note_profile)
        .await
        .unwrap();
    assert!(
        note_report.passed,
        "{}",
        serde_json::to_string(&note_report).unwrap()
    );
    service
        .set_calibration_maintenance(&context, false)
        .await
        .unwrap();
    assert!(
        service
            .calibration_status(&context, "memory")
            .await
            .unwrap()
            .active,
        "maintenance pause must preserve applicable query state"
    );
    assert!(
        service
            .execute_retrieval_calibration(&context, &profile)
            .await
            .is_err()
    );
    providers
        .set_provider_mode(&context, ProviderMode::Disabled, None)
        .await
        .unwrap();
    let disabled = service
        .calibration_status(&context, "memory")
        .await
        .unwrap();
    assert!(!disabled.active);
    assert!(disabled.blockers.contains(&"provider_disabled".into()));
    configure_test_embeddings(&state, &context).await;
    assert!(
        !service
            .calibration_status(&context, "memory")
            .await
            .unwrap()
            .active
    );
    assert!(
        service
            .execute_retrieval_calibration(&context, &profile)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn individually_passing_channels_cannot_bypass_joint_no_answer_gate() {
    let (_dir, state, context, _core, service) = fixture("joint-quality").await;
    let (providers, model) = configure_test_embeddings(&state, &context).await;
    providers
        .bind_model(Some(&context), "embedding_note", model, json!({}), None)
        .await
        .unwrap();
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/memory-quality/calibration.json"
    ))
    .unwrap();
    let failures: Vec<_> = corpus["queries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|q| q["split"] == "holdout" && q["relevant"].as_array().unwrap().is_empty())
        .map(|q| q["id"].as_str().unwrap().to_owned())
        .collect();
    for (index, channel) in ["memory", "note"].into_iter().enumerate() {
        let profile = service
            .calibration_profile(&context, channel)
            .await
            .unwrap()
            .unwrap();
        let mut report = service
            .execute_retrieval_calibration(&context, &profile)
            .await
            .unwrap();
        assert!(report.passed);
        // Test-only injection at the persisted-report boundary: two individually
        // legal 1/20 error sets are disjoint. No Admin API accepts these metrics.
        report.holdout.no_answer_false_returns = 1;
        report.holdout.no_answer_false_return_rate = 0.05;
        report.holdout.failed_cases = vec![failures[index].clone()];
        state
            .settings()
            .set_vault(
                &context,
                &format!(
                    "retrieval.calibration.active.{channel}.{}",
                    profile.signature
                ),
                &json!(report),
                mcp_vault_domain::WritePrecondition::Unconditional,
                None,
            )
            .await
            .unwrap();
    }
    for channel in ["memory", "note"] {
        let status = service.calibration_status(&context, channel).await.unwrap();
        assert!(!status.active);
        assert!(
            status.report.unwrap().passed,
            "keep the channel report for diagnosis"
        );
        assert!(
            status
                .blockers
                .contains(&"joint_no_answer_quality_failed".into())
        );
        assert_eq!(status.joint_no_answer.unwrap().false_return_rate, 0.10);
    }
    assert!(
        service
            .ensure_retrieval_calibration(&context)
            .await
            .unwrap()
            .is_none(),
        "joint quality failure must not create an automatic retry storm"
    );
}

#[tokio::test]
async fn generation_receives_full_source_language_and_coverage_contract_without_forced_reextract() {
    let (_dir, state, context, core, service) = fixture("generation-coverage").await;
    let model_state = Arc::new(CurrentSetModelState::default());
    model_state.mode.store(MODEL_EMPTY_SET, Ordering::SeqCst);
    configure_extraction(&state, &context, &service, model_state.clone()).await;
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/memory-quality/generation-coverage.json"
    ))
    .unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 15);
    for (i, case) in cases.iter().enumerate() {
        let path = VaultPath::parse(&format!("coverage-{i}.md")).unwrap();
        let source = case["source"].as_str().unwrap();
        core.create_bytes(
            &context,
            &path,
            source.as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        service.extract_note(&context, &core, &path).await.unwrap();
        let calls = model_state.calls.load(Ordering::SeqCst);
        service.extract_note(&context, &core, &path).await.unwrap();
        assert_eq!(
            model_state.calls.load(Ordering::SeqCst),
            calls,
            "unchanged current source cannot trigger full extraction"
        );
        let requests = model_state.requests.lock().await;
        let request = requests.last().unwrap();
        assert!(
            request["messages"][1]["content"]
                .as_str()
                .unwrap()
                .contains(source),
            "frontmatter, middle and final section must reach the real adapter"
        );
        let system = request["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("primary language"));
        assert!(system.contains("completed work"));
        assert!(system.contains("next planned stage"));
    }
}

#[tokio::test]
async fn diagnostic_threshold_does_not_suppress_similarity_candidates_or_lexical_answers() {
    let (_dir, state, context, core, service) = fixture("positive-hard-negative").await;
    let (providers, model) = configure_test_embeddings(&state, &context).await;
    let item = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: "orchard irrigation schedule".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    let current = state
        .current_memory()
        .get(&context, item.id)
        .await
        .unwrap()
        .unwrap();
    service
        .reembed_sources(
            &context,
            model,
            &[mcp_vault_providers::EmbeddingSourceRef {
                object_type: "memory".into(),
                object_id: item.id.to_string(),
                chunk_key: "body-v3:0000".into(),
                content_hash: current.memory.content_hash,
            }],
        )
        .await
        .unwrap();
    let row = state
        .providers()
        .list_embeddings(&context, model, "memory", 10, 0)
        .await
        .unwrap()
        .remove(0);
    let mut hard_negative = vec![0.0_f32; 64];
    hard_negative[60] = 0.2;
    hard_negative[61] = 0.96_f32.sqrt();
    state
        .providers()
        .upsert_embedding(&context, &row, &hard_negative)
        .await
        .unwrap();
    let profile = service
        .calibration_profile(&context, "memory")
        .await
        .unwrap()
        .unwrap();
    assert!(
        service
            .execute_retrieval_calibration(&context, &profile)
            .await
            .unwrap()
            .passed
    );
    providers
        .bind_model(Some(&context), "embedding_note", model, json!({}), None)
        .await
        .unwrap();
    let note_path = VaultPath::parse("orchard.md").unwrap();
    core.create_bytes(
        &context,
        &note_path,
        b"# Orchard
orchard irrigation schedule",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let index = mcp_vault_indexer::IndexService::with_provider_service(state.clone(), providers);
    index.rebuild_vault(&core, &context).await.unwrap();
    assert!(
        index
            .schedule_note_embeddings(&context)
            .await
            .unwrap()
            .source_chunks
            > 0
    );
    for job in state
        .jobs()
        .list(&context, None, Some("embedding.rebuild"), 100, 0)
        .await
        .unwrap()
    {
        let sources: Vec<mcp_vault_providers::EmbeddingSourceRef> =
            serde_json::from_value(job.payload["sources"].clone()).unwrap();
        let notes = sources
            .into_iter()
            .filter(|source| source.object_type == "note")
            .collect::<Vec<_>>();
        if !notes.is_empty() {
            index
                .reembed_note_sources(&context, model, &notes)
                .await
                .unwrap();
        }
    }
    let note_profile = service
        .calibration_profile(&context, "note")
        .await
        .unwrap()
        .unwrap();
    assert!(
        service
            .execute_retrieval_calibration(&context, &note_profile)
            .await
            .unwrap()
            .passed
    );
    let note_vectors = state
        .providers()
        .list_embeddings(&context, model, "note", 100, 0)
        .await
        .unwrap();
    assert!(!note_vectors.is_empty());
    for vector in note_vectors {
        state
            .providers()
            .upsert_embedding(&context, &vector, &hard_negative)
            .await
            .unwrap();
    }
    let unrelated = service
        .recall(
            &context,
            RecallRequest {
                query: "orchard telescope launch".into(),
                include_related_notes: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        !unrelated.memories.is_empty(),
        "similarity is not an answer guarantee"
    );
    assert!(!unrelated.related_notes.is_empty());
    let lexical = service
        .recall(
            &context,
            RecallRequest {
                query: "orchard irrigation schedule".into(),
                include_related_notes: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(lexical.memories[0].id, item.id);
    service
        .forget(&context, &core, item.id, item.revision)
        .await
        .unwrap();
    let note_only = service
        .recall(
            &context,
            RecallRequest {
                query: "orchard irrigation schedule".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(note_only.memories.is_empty());
    assert_eq!(note_only.related_notes.len(), 1);
    assert_eq!(note_only.related_notes[0].path, note_path);
}

#[tokio::test]
async fn source_resume_replays_canonical_commit_and_queues_extraction_exactly_once() {
    let (_dir, state, context, core, service) = fixture("resume-recovery").await;
    let model = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, model.clone()).await;
    let path = VaultPath::parse("resume.md").unwrap();
    let source = core
        .create_bytes(
            &context,
            &path,
            b"# FIRST\nAlpha backend",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    service.extract_note(&context, &core, &path).await.unwrap();
    let set = state
        .current_memory()
        .get_note_set_by_source(&context, source.id)
        .await
        .unwrap()
        .unwrap();
    let item = state
        .current_memory()
        .list_note_set_items(&context, set.id)
        .await
        .unwrap()
        .remove(0)
        .memory;
    service
        .forget(&context, &core, item.id, item.revision)
        .await
        .unwrap();
    let paused = state
        .current_memory()
        .get_note_set_by_source(&context, source.id)
        .await
        .unwrap()
        .unwrap();
    let failing = core
        .clone()
        .with_failure_injector(Arc::new(FailOnceAt::new(CommitPhase::MetadataCommitted)));
    let intent_id = service
        .resume_note_extraction(
            &context,
            &failing,
            source.id,
            paused.set_revision,
            Actor::system(),
        )
        .await
        .unwrap();
    let intent = state
        .jobs()
        .get(&context, intent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(intent.job_type, "memory.source_resume");
    assert!(
        state
            .current_memory()
            .get_note_set_by_source(&context, source.id)
            .await
            .unwrap()
            .unwrap()
            .extraction_paused
    );
    let calls = model.calls.load(Ordering::SeqCst);
    let extraction = service
        .complete_note_extraction_resume(&context, &core, &intent.payload)
        .await
        .unwrap();
    assert_eq!(
        state
            .jobs()
            .get(&context, extraction)
            .await
            .unwrap()
            .unwrap()
            .job_type,
        "memory.extract"
    );
    assert_eq!(
        service
            .complete_note_extraction_resume(&context, &core, &intent.payload)
            .await
            .unwrap(),
        extraction
    );
    assert!(
        !state
            .current_memory()
            .get_note_set_by_source(&context, source.id)
            .await
            .unwrap()
            .unwrap()
            .extraction_paused
    );
    assert_eq!(
        model.calls.load(Ordering::SeqCst),
        calls,
        "resume publication itself must not call a Provider"
    );
}

#[tokio::test]
async fn recall_revalidates_current_objects_after_related_note_provider_wait() {
    let (_dir, state, context, core, service) = fixture("final-current-check").await;
    let gate = Arc::new(CurrentSetModelState::default());
    let (providers, model) =
        configure_test_embeddings_with_gate(&state, &context, gate.clone()).await;
    providers
        .bind_model(Some(&context), "embedding_note", model, json!({}), None)
        .await
        .unwrap();
    let profile = service
        .calibration_profile(&context, "note")
        .await
        .unwrap()
        .unwrap();
    assert!(
        service
            .execute_retrieval_calibration(&context, &profile)
            .await
            .unwrap()
            .passed
    );
    let item = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: "orchard irrigation schedule".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    gate.mode.store(MODEL_BLOCKED, Ordering::SeqCst);
    let recall = tokio::spawn({
        let service = service.clone();
        let context = context.clone();
        async move {
            service
                .recall(
                    &context,
                    RecallRequest {
                        query: "orchard irrigation schedule".into(),
                        ..Default::default()
                    },
                )
                .await
                .unwrap()
        }
    });
    tokio::time::timeout(Duration::from_secs(5), gate.started.notified())
        .await
        .unwrap();
    service
        .forget(&context, &core, item.id, item.revision)
        .await
        .unwrap();
    gate.mode.store(0, Ordering::SeqCst);
    gate.release.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(10), recall)
        .await
        .unwrap()
        .unwrap();
    assert!(result.memories.is_empty());
    assert!(
        result
            .degraded
            .contains(&"candidate_changed_before_response".into())
    );
}

#[tokio::test]
async fn existing_v21_exact_duplicates_compact_without_extraction_and_rebuild_stays_compact() {
    check_existing_v21_exact_compaction(false).await;
}

#[tokio::test]
async fn exact_compaction_adopts_prepared_bytes_after_canonical_commit_failure() {
    check_existing_v21_exact_compaction(true).await;
}

async fn check_existing_v21_exact_compaction(inject_failure: bool) {
    let (_directory, state, context, core, service) = fixture("exact-upgrade-v21").await;
    let model = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, model.clone()).await;
    let path = VaultPath::parse("notes/upgrade.md").unwrap();
    let note_bytes = b"# FIRST\nAlpha requires Rust 1.94. Go was not adopted.";
    let source = core
        .create_bytes(
            &context,
            &path,
            note_bytes,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    service.extract_note(&context, &core, &path).await.unwrap();
    let set = state
        .current_memory()
        .get_note_set_by_source(&context, source.id)
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
    let survivor = items[0]["id"].as_str().unwrap().to_owned();
    let mut duplicate = items[0].clone();
    let absorbed = MemoryId::new();
    duplicate["id"] = json!(absorbed);
    duplicate["ordinal"] = json!(1);
    items[1]["ordinal"] = json!(2);
    items.insert(1, duplicate);
    let old_format = format!(
        "{}```json\n{}\n```{trailer}",
        header.replace("extraction_paused: false", "extraction_paused: true"),
        serde_json::to_string_pretty(&items).unwrap()
    );
    core.replace_managed_bytes(
        &context,
        &set.canonical_path,
        set.canonical_revision,
        old_format.as_bytes(),
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    // Fixture setup represents an already published v2.1 set with duplicates.
    assert_eq!(
        service.rebuild(&context, &core).await.unwrap().quarantined,
        0
    );
    assert_eq!(
        service
            .list(&context, vec![], None, None, None, 20, 0)
            .await
            .unwrap()
            .len(),
        3
    );
    let calls_before_upgrade = model.calls.load(Ordering::SeqCst);
    service
        .ensure_exact_memory_dedup_scheduled(&context)
        .await
        .unwrap();
    let jobs = state
        .jobs()
        .list(&context, None, Some("memory.deduplicate_source"), 20, 0)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    if inject_failure {
        let failing = core
            .clone()
            .with_failure_injector(Arc::new(FailOnceAt::new(CommitPhase::MetadataCommitted)));
        assert!(
            service
                .deduplicate_source_exact(&context, &failing, source.id)
                .await
                .is_err()
        );
        assert!(
            state
                .current_memory()
                .prepared_note_set_snapshot(&context, source.id)
                .await
                .unwrap()
                .is_some()
        );
    }
    assert_eq!(
        service
            .deduplicate_source_exact(&context, &core, source.id)
            .await
            .unwrap(),
        if inject_failure { 0 } else { 1 }
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), calls_before_upgrade);
    assert!(matches!(
        service.get(&context, absorbed).await,
        Err(MemoryError::NotFound)
    ));
    assert!(
        service
            .get(&context, MemoryId::parse(&survivor).unwrap())
            .await
            .is_ok()
    );
    assert_eq!(
        service
            .list(&context, vec![], None, None, None, 20, 0)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        state
            .current_memory()
            .get_note_set_by_source(&context, source.id)
            .await
            .unwrap()
            .unwrap()
            .extraction_paused
    );
    assert_eq!(
        service.rebuild(&context, &core).await.unwrap().quarantined,
        0
    );
    assert_eq!(
        service
            .list(&context, vec![], None, None, None, 20, 0)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        service
            .deduplicate_source_exact(&context, &core, source.id)
            .await
            .unwrap(),
        0
    );
    let mut original = core.read(&context, &path).await.unwrap();
    let mut actual = Vec::new();
    original.reader.read_to_end(&mut actual).await.unwrap();
    assert_eq!(actual, note_bytes);
}

#[tokio::test]
async fn equivalence_uses_existing_provider_and_persisted_cache_but_excludes_explicit() {
    let (_directory, state, context, core, service) = fixture("equivalence-provider").await;
    let model = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, model.clone()).await;
    let mut ids = Vec::new();
    for path in ["notes/first.md", "notes/second.md"] {
        let path = VaultPath::parse(path).unwrap();
        core.create_bytes(
            &context,
            &path,
            b"# FIRST\nAlpha requires Rust 1.94.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        service.extract_note(&context, &core, &path).await.unwrap();
        ids.push(
            service
                .list(&context, vec![], None, None, Some(path.to_string()), 20, 0)
                .await
                .unwrap()[0]
                .id,
        );
    }
    let calls = model.calls.load(Ordering::SeqCst);
    assert!(
        service
            .judge_memory_equivalence(&context, ids[0], ids[1])
            .await
            .unwrap()
            .permits_cross_source_merge()
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), calls + 1);
    assert!(
        service
            .judge_memory_equivalence(&context, ids[0], ids[1])
            .await
            .unwrap()
            .permits_cross_source_merge()
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), calls + 1);
    let explicit = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: "Alpha requires Rust 1.94.".into(),
                ..RememberInput::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    assert!(
        service
            .judge_memory_equivalence(&context, explicit.id, ids[0])
            .await
            .is_err()
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), calls + 1);
    // A judgment is not a publication; it must not silently change ownership.
    assert_eq!(
        service
            .list(&context, vec![], None, None, None, 20, 0)
            .await
            .unwrap()
            .len(),
        5
    );
}

#[tokio::test]
async fn formal_adoption_merge_forget_and_rebuild_keep_one_current_object() {
    let (_dir, state, context, core, service) = fixture("formal-roundtrip").await;
    let model = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, model.clone()).await;
    let mut ids = Vec::new();
    let mut sources = Vec::new();
    for name in ["a", "b"] {
        let path = VaultPath::parse(&format!("notes/{name}.md")).unwrap();
        let file = core
            .create_bytes(
                &context,
                &path,
                b"# FIRST\nAlpha requires Rust 1.94.",
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await
            .unwrap()
            .file;
        sources.push(file.id);
        service.extract_note(&context, &core, &path).await.unwrap();
        ids.push(
            service
                .list(&context, vec![], None, None, Some(path.to_string()), 20, 0)
                .await
                .unwrap()
                .into_iter()
                .find(|m| m.content.contains("requires Rust"))
                .unwrap()
                .id,
        );
    }
    let calls = model.calls.load(Ordering::SeqCst);
    assert_eq!(
        service
            .adopt_current_memory_sets(&context, &core)
            .await
            .unwrap(),
        4
    );
    assert_eq!(calls, model.calls.load(Ordering::SeqCst));
    assert!(
        state
            .current_memory()
            .formal_enabled(&context)
            .await
            .unwrap()
    );
    assert!(
        service
            .merge_equivalent_formal_memories(&context, &core, ids[0], ids[1])
            .await
            .unwrap()
    );
    let all = service
        .list(&context, vec![], None, None, None, 20, 0)
        .await
        .unwrap();
    assert_eq!(all.len(), 3);
    let merged = all
        .iter()
        .find(|m| m.content.contains("requires Rust"))
        .unwrap();
    assert_eq!(merged.sources.len(), 2);
    assert!(merged.note_set_id.is_none());
    assert!(
        merged
            .canonical_path
            .as_ref()
            .unwrap()
            .as_str()
            .contains("/facts/")
    );
    for path in ["notes/a.md", "notes/b.md"] {
        let filtered = service
            .list(&context, vec![], None, None, Some(path.into()), 20, 0)
            .await
            .unwrap();
        assert_eq!(filtered.iter().filter(|m| m.id == merged.id).count(), 1);
    }
    let absorbed = *ids.iter().find(|id| **id != merged.id).unwrap();
    assert!(matches!(
        service.get(&context, absorbed).await,
        Err(MemoryError::NotFound)
    ));
    assert!(matches!(
        service
            .forget(&context, &core, absorbed, Revision::new(1))
            .await,
        Err(MemoryError::NotFound)
    ));
    assert_eq!(
        service.rebuild(&context, &core).await.unwrap().quarantined,
        0
    );
    assert_eq!(
        service
            .get(&context, merged.id)
            .await
            .unwrap()
            .sources
            .len(),
        2
    );
    service
        .forget(&context, &core, merged.id, merged.revision)
        .await
        .unwrap();
    assert!(matches!(
        service.get(&context, merged.id).await,
        Err(MemoryError::NotFound)
    ));
    assert_eq!(
        service.rebuild(&context, &core).await.unwrap().quarantined,
        0
    );
    assert_eq!(
        service
            .list(&context, vec![], None, None, None, 20, 0)
            .await
            .unwrap()
            .len(),
        2
    );
    for source in sources {
        assert!(
            state
                .current_memory()
                .get_note_set_by_source(&context, source)
                .await
                .unwrap()
                .unwrap()
                .extraction_paused
        );
    }
}

async fn two_formal_sources(
    slug: &str,
) -> (
    TempDir,
    StateStore,
    VaultContext,
    VaultCore,
    MemoryService,
    Arc<CurrentSetModelState>,
    Vec<mcp_vault_state::FileRecord>,
    Vec<MemoryId>,
) {
    let (dir, state, context, core, service) = fixture(slug).await;
    let model = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, model.clone()).await;
    let mut files = Vec::new();
    let mut ids = Vec::new();
    for name in ["a", "b"] {
        let path = VaultPath::parse(&format!("notes/{name}.md")).unwrap();
        files.push(
            core.create_bytes(
                &context,
                &path,
                b"# FIRST\nAlpha requires Rust 1.94.",
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await
            .unwrap()
            .file,
        );
        service.extract_note(&context, &core, &path).await.unwrap();
        ids.push(
            service
                .list(&context, vec![], None, None, Some(path.to_string()), 20, 0)
                .await
                .unwrap()
                .into_iter()
                .find(|m| m.content.contains("requires Rust"))
                .unwrap()
                .id,
        );
    }
    service
        .adopt_current_memory_sets(&context, &core)
        .await
        .unwrap();
    (dir, state, context, core, service, model, files, ids)
}

#[tokio::test]
async fn formal_support_loss_is_immediate_and_local_cleanup_needs_no_model() {
    let (_dir, state, context, core, service, model, files, ids) =
        two_formal_sources("formal-support-loss").await;
    assert!(
        service
            .merge_equivalent_formal_memories(&context, &core, ids[0], ids[1])
            .await
            .unwrap()
    );
    let formal = service
        .list(&context, vec![], None, None, None, 20, 0)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.content.contains("requires Rust"))
        .unwrap();
    let policy = service.extraction_policy(&context).await.unwrap();
    service
        .set_extraction_policy(
            &context,
            ExtractionPolicy {
                enabled: false,
                ..policy.policy
            },
            policy.revision,
            None,
        )
        .await
        .unwrap();
    let calls = model.calls.load(Ordering::SeqCst);
    for (index, file) in files.iter().enumerate() {
        core.delete(
            &context,
            &file.path,
            file.current_revision,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        if index == 0 {
            let current = service.get(&context, formal.id).await.unwrap();
            assert_eq!(current.sources.len(), 1);
            assert_eq!(current.sources[0].file_id, Some(files[1].id));
        } else {
            assert!(matches!(
                service.get(&context, formal.id).await,
                Err(MemoryError::NotFound)
            ));
        }
        service
            .maintain_memory_dedup(&context, &core)
            .await
            .unwrap();
    }
    assert_eq!(calls, model.calls.load(Ordering::SeqCst));
    assert!(
        state
            .current_memory()
            .formal_document(&context, formal.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        service.rebuild(&context, &core).await.unwrap().quarantined,
        0
    );
    assert!(
        service
            .list(&context, vec![], None, None, None, 20, 0)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn formal_merge_and_forget_recover_after_canonical_commit_without_resurrection() {
    for forgetting in [false, true] {
        let (_dir, state, context, core, service, model, _files, ids) =
            two_formal_sources("formal-crash").await;
        let failure = core
            .clone()
            .with_failure_injector(Arc::new(FailOnceAt::new(CommitPhase::MetadataCommitted)));
        let id = if forgetting {
            service
                .merge_equivalent_formal_memories(&context, &core, ids[0], ids[1])
                .await
                .unwrap();
            let current = service
                .list(&context, vec![], None, None, None, 20, 0)
                .await
                .unwrap()
                .into_iter()
                .find(|m| m.content.contains("requires Rust"))
                .unwrap();
            assert!(
                service
                    .forget(&context, &failure, current.id, current.revision)
                    .await
                    .is_err()
            );
            assert!(matches!(
                service.get(&context, current.id).await,
                Err(MemoryError::NotFound)
            ));
            current.id
        } else {
            assert!(
                service
                    .merge_equivalent_formal_memories(&context, &failure, ids[0], ids[1])
                    .await
                    .is_err()
            );
            assert_eq!(
                service
                    .list(&context, vec![], None, None, None, 20, 0)
                    .await
                    .unwrap()
                    .len(),
                4
            );
            ids[0]
        };
        assert!(
            state
                .current_memory()
                .formal_operation(&context)
                .await
                .unwrap()
                .is_some()
        );
        let calls = model.calls.load(Ordering::SeqCst);
        let restarted = MemoryService::new(
            state.clone(),
            AuthService::new(
                state.auth(),
                MasterKeyRing::from_bytes(1, &[23; 32]).unwrap(),
            ),
        );
        restarted
            .recover_formal_publication(&context, &core)
            .await
            .unwrap();
        restarted
            .recover_formal_publication(&context, &core)
            .await
            .unwrap();
        assert!(
            state
                .current_memory()
                .formal_operation(&context)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            restarted
                .rebuild(&context, &core)
                .await
                .unwrap()
                .quarantined,
            0
        );
        assert_eq!(model.calls.load(Ordering::SeqCst), calls);
        let all = restarted
            .list(&context, vec![], None, None, None, 20, 0)
            .await
            .unwrap();
        assert_eq!(all.len(), if forgetting { 2 } else { 3 });
        if forgetting {
            assert!(matches!(
                restarted.get(&context, id).await,
                Err(MemoryError::NotFound)
            ));
        }
    }
}

#[tokio::test]
async fn newly_extracted_memory_merges_before_ten_thousand_historical_pairs_after_restart() {
    let (_dir, state, context, core, service) = fixture("fresh-before-backlog").await;
    let model = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, model.clone()).await;
    for i in 0..2 {
        let path = VaultPath::parse(&format!("backlog/{i}.md")).unwrap();
        core.create_bytes(
            &context,
            &path,
            format!("# FAIRNESS:{i}\nIndependent project {i}.").as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        service.extract_note(&context, &core, &path).await.unwrap();
    }
    for _ in 0..10 {
        if !service
            .maintain_memory_dedup(&context, &core)
            .await
            .unwrap()
        {
            break;
        }
    }
    let old = state
        .current_memory()
        .contributions(&context, None, 10)
        .await
        .unwrap();
    assert_eq!(old.len(), 2);
    // Simulate a large persistent historical queue; all sort before hashed fresh keys.
    let backlog = (0..10_000)
        .map(|i| {
            (
                format!("000-backlog-{i:05}"),
                old[0].memory.id,
                old[1].memory.id,
            )
        })
        .collect::<Vec<_>>();
    state
        .current_memory()
        .queue_formal_pairs(&context, MemoryId::new(), "historical", &backlog)
        .await
        .unwrap();
    let path = VaultPath::parse("new/duplicate.md").unwrap();
    let note = b"# FAIRNESS:0\nIndependent project 0.";
    core.create_bytes(
        &context,
        &path,
        note,
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    service.extract_note(&context, &core, &path).await.unwrap();
    assert_eq!(
        state
            .current_memory()
            .new_dedup_contributions(&context, 16)
            .await
            .unwrap()
            .len(),
        1
    );
    let restarted = MemoryService::new(
        state.clone(),
        AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[23; 32]).unwrap(),
        ),
    );
    assert!(
        restarted
            .maintain_memory_dedup(&context, &core)
            .await
            .unwrap()
    );
    let visible = restarted
        .list(&context, vec![], None, None, None, 20, 0)
        .await
        .unwrap();
    assert_eq!(
        visible.len(),
        2,
        "fresh duplicate must merge without draining historical backlog"
    );
    let status = state
        .current_memory()
        .formal_status(&context)
        .await
        .unwrap();
    assert!(status.pending_pairs >= 10_000);
    assert!(
        state
            .current_memory()
            .new_dedup_contributions(&context, 16)
            .await
            .unwrap()
            .is_empty()
    );
    let mut read = core.read(&context, &path).await.unwrap();
    let mut bytes = Vec::new();
    read.reader.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(bytes, note);
}

#[tokio::test]
async fn interrupted_pair_does_not_block_later_pairs_after_restart() {
    let (_dir, state, context, core, service) = fixture("pair-rotation").await;
    let model = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, model.clone()).await;
    for i in 0..3 {
        let path = VaultPath::parse(&format!("rotation/{i}.md")).unwrap();
        core.create_bytes(
            &context,
            &path,
            format!("# FAIRNESS:{i}\nIndependent project {i}.").as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        service.extract_note(&context, &core, &path).await.unwrap();
    }
    model.mode.store(MODEL_JUDGMENT_BLOCKED, Ordering::SeqCst);
    let mut attempted = Vec::new();
    for _ in 0..3 {
        let restarted = MemoryService::new(
            state.clone(),
            AuthService::new(
                state.auth(),
                MasterKeyRing::from_bytes(1, &[23; 32]).unwrap(),
            ),
        );
        let work = tokio::spawn({
            let context = context.clone();
            let core = core.clone();
            async move { restarted.maintain_memory_dedup(&context, &core).await }
        });
        tokio::time::timeout(Duration::from_secs(10), model.started.notified())
            .await
            .unwrap();
        let request = model.requests.lock().await.last().unwrap().clone();
        let pair: Value =
            serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
        attempted.push((
            pair["left"]["content"].clone(),
            pair["right"]["content"].clone(),
        ));
        work.abort();
        assert!(work.await.unwrap_err().is_cancelled());
        model.release.notify_waiters();
    }
    assert_ne!(
        attempted[0], attempted[1],
        "restart repeatedly selected the same unfinished pair"
    );
    assert_ne!(attempted[0], attempted[2]);
    assert_ne!(attempted[1], attempted[2]);
    let status = state
        .current_memory()
        .formal_status(&context)
        .await
        .unwrap();
    assert_eq!(
        status.pending_pairs, 3,
        "interrupted pairs must remain pending, not become fake successes"
    );
    assert_eq!(status.checked_pairs, 0);
    model.mode.store(MODEL_NORMAL, Ordering::SeqCst);
    for _ in 0..10 {
        if !service
            .maintain_memory_dedup(&context, &core)
            .await
            .unwrap()
        {
            break;
        }
    }
    assert_eq!(
        state
            .current_memory()
            .formal_status(&context)
            .await
            .unwrap()
            .pending_pairs,
        0
    );
    assert_eq!(
        service
            .list(&context, vec![], None, None, None, 20, 0)
            .await
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn unchanged_memory_bodies_cannot_starve_pairs_and_interrupted_checks_resume_after_cursor() {
    let (_dir, state, context, core, service) = fixture("dedup-fairness").await;
    let model = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, model.clone()).await;
    let mut originals = Vec::new();
    for i in 0..20 {
        let path = VaultPath::parse(&format!("fairness/{i}.md")).unwrap();
        let bytes =
            format!("# FAIRNESS:{i}\nProject {i} uses Rust. Batch size is 32. Go was not adopted.");
        core.create_bytes(
            &context,
            &path,
            bytes.as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        service.extract_note(&context, &core, &path).await.unwrap();
        originals.push((path, bytes));
    }
    let before = model.calls.load(Ordering::SeqCst);
    assert!(
        service
            .maintain_memory_dedup(&context, &core)
            .await
            .unwrap()
    );
    let progress = state
        .current_memory()
        .formal_status(&context)
        .await
        .unwrap();
    assert!(
        progress.checked_pairs > 0,
        "unchanged bodies must not consume every slice before pairs run"
    );
    assert!(progress.pending_pairs > 0);
    assert_eq!(
        progress.sentence_checked, 2,
        "bound checks, not only successful rewrites"
    );
    assert!(model.calls.load(Ordering::SeqCst) - before <= 6);
    model.mode.store(MODEL_SENTENCE_BLOCKED, Ordering::SeqCst);
    let interrupted = tokio::spawn({
        let service = service.clone();
        let context = context.clone();
        let core = core.clone();
        async move { service.maintain_memory_dedup(&context, &core).await }
    });
    tokio::time::timeout(Duration::from_secs(10), model.started.notified())
        .await
        .unwrap();
    let cursor = state
        .current_memory()
        .sentence_scan_cursor(&context)
        .await
        .unwrap()
        .unwrap();
    let during = state
        .current_memory()
        .formal_status(&context)
        .await
        .unwrap();
    assert_eq!(during.phase, "checking_sentences");
    assert!(
        during.checked_pairs > progress.checked_pairs,
        "pairs advance even before a slow sentence check finishes"
    );
    interrupted.abort();
    assert!(interrupted.await.unwrap_err().is_cancelled());
    model.mode.store(MODEL_NORMAL, Ordering::SeqCst);
    model.release.notify_waiters();
    let restarted = MemoryService::new(
        state.clone(),
        AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[23; 32]).unwrap(),
        ),
    );
    assert!(
        restarted
            .maintain_memory_dedup(&context, &core)
            .await
            .unwrap()
    );
    assert!(
        state
            .current_memory()
            .sentence_scan_cursor(&context)
            .await
            .unwrap()
            .unwrap()
            > cursor
    );
    assert_eq!(
        state
            .current_memory()
            .formal_status(&context)
            .await
            .unwrap()
            .sentence_checked,
        4
    );
    for (path, bytes) in originals {
        let mut read = core.read(&context, &path).await.unwrap();
        let mut actual = Vec::new();
        read.reader.read_to_end(&mut actual).await.unwrap();
        assert_eq!(actual, bytes.as_bytes());
    }
}

#[tokio::test]
async fn dedup_labeled_60_pairs_and_20_queries_preserve_boundaries_and_paginate_over_100_sources() {
    let (_dir, state, context, core, service) = fixture("dedup-labeled").await;
    let model = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, model.clone()).await;
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/memory-quality/dedup-merge.json"
    ))
    .unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 60);
    let mut pairs = Vec::new();
    for case in cases {
        let name = case["id"].as_str().unwrap();
        let mut ids = Vec::new();
        for side in ["left", "right"] {
            let path = VaultPath::parse(&format!("dedup/{name}-{side}.md")).unwrap();
            core.create_bytes(
                &context,
                &path,
                format!(
                    "# DEDUP_CASE {name}:{side}\n{}",
                    case[side].as_str().unwrap()
                )
                .as_bytes(),
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await
            .unwrap();
            service.extract_note(&context, &core, &path).await.unwrap();
            ids.push(
                service
                    .list(&context, vec![], None, None, Some(path.to_string()), 20, 0)
                    .await
                    .unwrap()[0]
                    .id,
            );
        }
        pairs.push((ids[0], ids[1]));
    }
    let calls = model.calls.load(Ordering::SeqCst);
    assert_eq!(
        service
            .adopt_current_memory_sets(&context, &core)
            .await
            .unwrap(),
        120
    );
    assert_eq!(calls, model.calls.load(Ordering::SeqCst));
    assert_eq!(
        service
            .list(&context, vec![], None, None, None, 100, 100)
            .await
            .unwrap()
            .len(),
        20
    );
    let mut report = Vec::new();
    for (case, (a, b)) in cases.iter().zip(pairs) {
        let relation = service
            .judge_memory_equivalence(&context, a, b)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(relation).unwrap(),
            case["relation"],
            "{}",
            case["id"]
        );
        let merged = service
            .merge_equivalent_formal_memories(&context, &core, a, b)
            .await
            .unwrap();
        assert_eq!(merged, case["relation"] == "equivalent", "{}", case["id"]);
        if !merged {
            assert!(service.get(&context, a).await.is_ok());
            assert!(service.get(&context, b).await.is_ok());
        }
        report.push(json!({"id":case["id"],"relation":relation,"merged":merged,"passed":true}));
    }
    let mut query_report = Vec::new();
    for query in corpus["queries"].as_array().unwrap() {
        let source = format!("dedup/{}-left.md", query["case"].as_str().unwrap());
        let expected = service
            .list(&context, vec![], None, None, Some(source), 20, 0)
            .await
            .unwrap()[0]
            .id;
        let result = service
            .recall(
                &context,
                RecallRequest {
                    query: query["query"].as_str().unwrap().into(),
                    max_results: 100,
                    max_tokens: 32_000,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(
            result.memories.iter().any(|m| m.id == expected),
            "missing query {}",
            query["id"]
        );
        let unique = result
            .memories
            .iter()
            .map(|m| m.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), result.memories.len());
        query_report.push(json!({"id":query["id"],"passed":true}));
    }
    assert_eq!(query_report.len(), 20);
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/automatic-memory-dedup-validation");
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(output.join("labeled-mechanism.json"),serde_json::to_vec_pretty(&json!({"mode":"local_fake_mechanism_not_real_model_quality","cases":report,"queries":query_report})).unwrap()).unwrap();
}

#[tokio::test]
async fn pairwise_chain_cannot_import_an_unverified_group_member() {
    let (_dir, state, context, core, service) = fixture("no-transitive-merge").await;
    let model = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, model.clone()).await;
    let mut ids = Vec::new();
    for marker in ["FIRST", "SECOND", "THIRD"] {
        let path = VaultPath::parse(&format!("notes/{marker}.md")).unwrap();
        core.create_bytes(
            &context,
            &path,
            marker.as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        service.extract_note(&context, &core, &path).await.unwrap();
        ids.push(
            service
                .list(&context, vec![], None, None, Some(path.to_string()), 20, 0)
                .await
                .unwrap()
                .into_iter()
                .find(|m| m.content.contains("requires"))
                .unwrap()
                .id,
        );
    }
    service
        .adopt_current_memory_sets(&context, &core)
        .await
        .unwrap();
    model.mode.store(MODEL_CHAIN, Ordering::SeqCst);
    assert!(
        service
            .merge_equivalent_formal_memories(&context, &core, ids[1], ids[2])
            .await
            .unwrap()
    );
    // The fake deliberately emits A≈B and B≈C but A≠C. The engine must
    // challenge C against A's complete body instead of taking graph closure.
    assert!(
        !service
            .merge_equivalent_formal_memories(&context, &core, ids[0], ids[1])
            .await
            .unwrap()
    );
    assert_eq!(
        service.get(&context, ids[0]).await.unwrap().sources.len(),
        1
    );
    assert_eq!(
        service.get(&context, ids[1]).await.unwrap().sources.len(),
        2
    );
}

#[tokio::test]
async fn source_edit_while_judgment_waits_cannot_publish_stale_support() {
    let (_dir, _state, context, core, service, model, files, ids) =
        two_formal_sources("formal-edit-race").await;
    model.mode.store(MODEL_JUDGMENT_BLOCKED, Ordering::SeqCst);
    let merging = tokio::spawn({
        let service = service.clone();
        let context = context.clone();
        let core = core.clone();
        let ids = ids.clone();
        async move {
            service
                .merge_equivalent_formal_memories(&context, &core, ids[0], ids[1])
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), model.started.notified())
        .await
        .unwrap();
    core.replace_bytes(
        &context,
        &files[0].path,
        files[0].current_revision,
        b"# SECOND\nThe backend now uses a different compiler.",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    model.release.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(5), merging)
        .await
        .unwrap()
        .unwrap();
    assert!(!matches!(result, Ok(true)));
    assert_eq!(
        service.get(&context, ids[1]).await.unwrap().sources.len(),
        1
    );
    assert!(matches!(
        service.get(&context, ids[0]).await,
        Err(MemoryError::NotFound)
    ));
}

#[tokio::test]
async fn same_source_inclusion_never_upgrades_another_sources_short_support() {
    let (_dir, state, context, core, service) = fixture("inclusion-provenance").await;
    let model = Arc::new(CurrentSetModelState::default());
    configure_extraction(&state, &context, &service, model).await;
    for (path, body) in [
        ("notes/a.md", "DEDUP_CONTAINED"),
        ("notes/b.md", "DEDUP_CASE D25F:right"),
    ] {
        let path = VaultPath::parse(path).unwrap();
        core.create_bytes(
            &context,
            &path,
            body.as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        service.extract_note(&context, &core, &path).await.unwrap();
    }
    for _ in 0..8 {
        if !service
            .maintain_memory_dedup(&context, &core)
            .await
            .unwrap()
        {
            break;
        }
    }
    let a = service
        .list(
            &context,
            vec![],
            None,
            None,
            Some("notes/a.md".into()),
            20,
            0,
        )
        .await
        .unwrap();
    let b = service
        .list(
            &context,
            vec![],
            None,
            None,
            Some("notes/b.md".into()),
            20,
            0,
        )
        .await
        .unwrap();
    assert_eq!(a.len(), 1);
    assert!(a[0].content.contains("formula"));
    assert_eq!(a[0].sources.len(), 1);
    assert_eq!(b.len(), 1);
    assert!(!b[0].content.contains("formula"));
    assert_eq!(b[0].sources.len(), 1);
    assert_ne!(a[0].id, b[0].id);
}

#[tokio::test]
async fn changed_judging_profile_splits_and_revalidates_existing_groups_automatically() {
    let (_dir, state, context, core, service, model, _files, ids) =
        two_formal_sources("group-revalidation").await;
    service
        .merge_equivalent_formal_memories(&context, &core, ids[0], ids[1])
        .await
        .unwrap();
    model.mode.store(MODEL_ALL_DIFFERENT, Ordering::SeqCst);
    let binding = state
        .providers()
        .resolve_binding(&context, "memory_extraction")
        .await
        .unwrap()
        .unwrap();
    let providers = ProviderService::new(
        state.clone(),
        AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[23; 32]).unwrap(),
        ),
    );
    providers
        .bind_model(
            Some(&context),
            "memory_extraction",
            binding.model_id,
            json!({"temperature":0.1}),
            Some(binding.revision),
        )
        .await
        .unwrap();
    for _ in 0..8 {
        if !service
            .maintain_memory_dedup(&context, &core)
            .await
            .unwrap()
        {
            break;
        }
    }
    let all = service
        .list(&context, vec![], None, None, None, 20, 0)
        .await
        .unwrap();
    assert_eq!(all.len(), 4);
    assert!(all.iter().all(|m| m.sources.len() == 1));
}

#[tokio::test]
async fn covered_memories_do_not_enqueue_again_until_new_contributions_arrive() {
    let (_dir, state, context, core, service, model, _files, _ids) =
        two_formal_sources("idle-admission").await;
    // Complete work and settle checkpoints after canonical changes from the first pass.
    for _ in 0..20 {
        service
            .maintain_memory_dedup(&context, &core)
            .await
            .unwrap();
        if state
            .current_memory()
            .formal_status(&context)
            .await
            .unwrap()
            .pending_pairs
            == 0
        {
            service
                .maintain_memory_dedup(&context, &core)
                .await
                .unwrap();
            break;
        }
    }
    state
        .jobs()
        .request_cancel_type(&context, "memory.deduplicate")
        .await
        .unwrap();
    let calls = model.calls.load(Ordering::SeqCst);
    let jobs_before = state
        .jobs()
        .list(&context, None, Some("memory.deduplicate"), 100, 0)
        .await
        .unwrap()
        .len();
    for _ in 0..3 {
        let restarted = MemoryService::new(
            state.clone(),
            AuthService::new(
                state.auth(),
                MasterKeyRing::from_bytes(1, &[23; 32]).unwrap(),
            ),
        );
        restarted
            .ensure_memory_dedup_scheduled(&context)
            .await
            .unwrap();
    }
    assert_eq!(
        state
            .jobs()
            .list(&context, None, Some("memory.deduplicate"), 100, 0)
            .await
            .unwrap()
            .len(),
        jobs_before
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), calls);
    let path = VaultPath::parse("new/trigger.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# FIRST\nAlpha requires Rust 1.94.",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    service.extract_note(&context, &core, &path).await.unwrap();
    assert!(
        state
            .jobs()
            .list(&context, None, Some("memory.deduplicate"), 100, 0)
            .await
            .unwrap()
            .len()
            > jobs_before
    );
}

#[tokio::test]
async fn malformed_pair_is_retained_without_repeated_paid_judgment() {
    let (_dir, state, context, core, service, model, _files, ids) =
        two_formal_sources("bad-pair-cache").await;
    model.mode.store(MODEL_INVALID_PAIR, Ordering::SeqCst);
    assert_eq!(
        service
            .judge_memory_equivalence(&context, ids[0], ids[1])
            .await
            .unwrap(),
        mcp_vault_memory::MemoryRelation::Uncertain
    );
    let calls = model.calls.load(Ordering::SeqCst);
    assert_eq!(
        service
            .judge_memory_equivalence(&context, ids[0], ids[1])
            .await
            .unwrap(),
        mcp_vault_memory::MemoryRelation::Uncertain
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), calls);
    assert_eq!(
        service
            .list(&context, vec![], None, None, None, 20, 0)
            .await
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        service.rebuild(&context, &core).await.unwrap().quarantined,
        0
    );
    for _ in 0..12 {
        if !service
            .maintain_memory_dedup(&context, &core)
            .await
            .unwrap()
        {
            break;
        }
    }
    assert_eq!(
        state
            .current_memory()
            .formal_status(&context)
            .await
            .unwrap()
            .pending_pairs,
        0
    );
}

#[tokio::test]
async fn regenerated_contribution_cannot_reuse_an_absorbed_formal_id() {
    let (_dir, state, context, core, service, _model, files, ids) =
        two_formal_sources("absorbed-id-reservation").await;
    service
        .merge_equivalent_formal_memories(&context, &core, ids[0], ids[1])
        .await
        .unwrap();
    assert!(matches!(
        service.get(&context, ids[1]).await,
        Err(MemoryError::NotFound)
    ));
    core.replace_bytes(
        &context,
        &files[1].path,
        files[1].current_revision,
        b"# FIRST\nSame compiler fact with additional source context.",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    service
        .extract_note(&context, &core, &files[1].path)
        .await
        .unwrap();
    service
        .reconcile_formal_supports(&context, &core)
        .await
        .unwrap();
    service
        .adopt_current_memory_sets(&context, &core)
        .await
        .unwrap();
    assert!(matches!(
        service.get(&context, ids[1]).await,
        Err(MemoryError::NotFound)
    ));
    let owner = state
        .current_memory()
        .contribution_owner(&context, ids[1])
        .await
        .unwrap()
        .unwrap();
    assert_ne!(owner, ids[1]);
    assert_eq!(
        service.rebuild(&context, &core).await.unwrap().quarantined,
        0
    );
    assert!(matches!(
        service.get(&context, ids[1]).await,
        Err(MemoryError::NotFound)
    ));
}
