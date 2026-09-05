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
    let memories = if mode == MODEL_EMPTY_SET {
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
                max_tokens: 128,
                ..RecallRequest::default()
            },
        )
        .await
        .unwrap();
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
                max_tokens: 220,
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
