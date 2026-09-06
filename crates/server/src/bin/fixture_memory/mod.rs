//! Opt-in disposable Admin browser fixture. Never loaded by production server.
use axum::{Json, Router, routing::post};
use mcp_vault_auth::{AuthService, SecretString};
use mcp_vault_core::{VaultCore, VaultCoreRuntime};
use mcp_vault_domain::{
    Actor, MemoryId, MemorySourceId, Revision, SourcePlane, VaultContext, VaultPath,
};
use mcp_vault_memory::{ExtractionPolicy, MemoryService, RememberInput};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderMode,
    ProviderService, ProviderSettings,
};
use mcp_vault_server::workers::{
    Cancellation, WorkerConfig, WorkerSupervisor, memory_extract_job_handler,
    retrieval_calibration_job_handler,
};
use mcp_vault_state::{MemoryBundle, MemoryRecord, MemorySourceRecord, StateStore};
use serde_json::{Value, json};
use std::{error::Error, path::Path, sync::Arc};

pub async fn prepare(
    state: &StateStore,
    context: &VaultContext,
    auth: &AuthService,
    providers: &ProviderService,
    memory: &MemoryService,
    history: &Path,
    runtime: &VaultCoreRuntime,
) -> Result<(), Box<dyn Error>> {
    auth.setup_admin(
        "browser-admin",
        &SecretString::new("isolated-browser-password-123"),
    )
    .await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/v1/embeddings", post(embeddings))
                .route("/v1/chat/completions", post(extract)),
        )
        .await
        .unwrap();
    });
    providers
        .set_provider_mode(context, ProviderMode::LocalOnly, None)
        .await?;
    let provider = providers
        .create_provider(ProviderInput {
            name: "synthetic browser contract".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: url::Url::parse(&format!("http://{address}/v1/"))?,
            settings: ProviderSettings::default(),
            enabled: true,
            secret: None,
        })
        .await?;
    let model = providers
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "browser-synthetic-model".into(),
            capabilities: ModelCapabilities {
                embeddings: true,
                dimension: Some(64),
                structured_output: true,
                max_output_tokens: Some(8192),
                ..Default::default()
            },
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await?;
    providers
        .bind_model(
            Some(context),
            "memory_extraction",
            model.id,
            json!({}),
            None,
        )
        .await?;
    memory
        .set_extraction_policy(
            context,
            ExtractionPolicy {
                enabled: true,
                ..Default::default()
            },
            None,
            None,
        )
        .await?;
    let core = VaultCore::new(
        state.clone(),
        history.to_owned(),
        Default::default(),
        Default::default(),
        runtime.clone(),
    );
    for i in 0..52 {
        memory
            .remember(
                context,
                &core,
                RememberInput {
                    content: format!("Browser explicit fixture {i:02}"),
                    tags: vec!["preserve-tag".into()],
                    importance: Some(0.8),
                    ..Default::default()
                },
            )
            .await?;
    }
    let path = VaultPath::parse("browser-source.md")?;
    core.create_bytes(
        context,
        &path,
        b"# Browser source\nThe synthetic team completed its local exercise.",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await?;
    memory.extract_note(context, &core, &path).await?;
    let id = MemoryId::new();
    state
        .memory()
        .replace_bundle(
            context,
            &MemoryBundle {
                memory: MemoryRecord {
                    id,
                    vault_id: context.id(),
                    memory_type: "fact".into(),
                    status: "active".into(),
                    status_reason: None,
                    status_changed_at: None,
                    content: "Legacy browser assertion".into(),
                    normalized_content: "legacy browser assertion".into(),
                    content_hash: "sha256:synthetic-legacy".into(),
                    importance: 0.7,
                    confidence: 0.8,
                    origin: "explicit_admin".into(),
                    revision: Revision::new(1),
                    canonical_file_id: None,
                    canonical_path: None,
                    canonical_revision: None,
                    valid_from: None,
                    valid_to: None,
                    extraction: json!({}),
                    created_at: 1,
                    updated_at: 1,
                    last_recalled_at: None,
                    recall_count: 0,
                },
                sources: vec![MemorySourceRecord {
                    id: MemorySourceId::new(),
                    vault_id: context.id(),
                    memory_id: id,
                    source_type: "explicit_admin".into(),
                    note_file_id: None,
                    note_path: None,
                    note_revision: None,
                    heading_path: vec![],
                    start_line: None,
                    end_line: None,
                    excerpt_hash: None,
                    actor_id: None,
                    created_at: 1,
                }],
                entities: vec![],
                tags: vec![],
                relations: vec![],
            },
            None,
        )
        .await?;
    providers
        .bind_model(Some(context), "embedding_memory", model.id, json!({}), None)
        .await?;
    // Seed complete valid vectors directly after creating the current records,
    // without pending embedding jobs. This isolates the missing-calibration UI
    // state while the fixture retains all real production handlers.
    let sources = state
        .current_memory()
        .list(context, &Default::default(), 100, 0)
        .await?
        .into_iter()
        .map(|memory| mcp_vault_providers::EmbeddingSourceRef {
            object_type: "memory".into(),
            object_id: memory.id.to_string(),
            chunk_key: "body-v3:0000".into(),
            content_hash: memory.content_hash,
        })
        .collect::<Vec<_>>();
    memory.reembed_sources(context, model.id, &sources).await?;
    let supervisor = WorkerSupervisor::new(
        state.clone(),
        Arc::new(|_| Box::pin(async { Ok(()) })),
        WorkerConfig::default(),
    )
    .map_err(|_| "fixture worker invalid")?;
    supervisor
        .register_job_handler(
            "retrieval.calibrate",
            retrieval_calibration_job_handler(state.clone(), memory.clone()),
        )
        .map_err(|_| "fixture registration invalid")?;
    supervisor
        .register_job_handler(
            "memory.extract",
            memory_extract_job_handler(
                state.clone(),
                history.to_owned(),
                runtime.clone(),
                memory.clone(),
            ),
        )
        .map_err(|_| "fixture registration invalid")?;
    supervisor
        .register_job_handler(
            "embedding.rebuild",
            mcp_vault_server::workers::embedding_job_handler(
                state.clone(),
                mcp_vault_indexer::IndexService::with_provider_service(
                    state.clone(),
                    providers.clone(),
                ),
                memory.clone(),
            ),
        )
        .map_err(|_| "fixture registration invalid")?;
    supervisor
        .register_job_handler(
            "memory.source_resume",
            mcp_vault_server::workers::memory_source_resume_job_handler(
                state.clone(),
                history.to_owned(),
                runtime.clone(),
                memory.clone(),
            ),
        )
        .map_err(|_| "fixture registration invalid")?;
    tokio::spawn(async move {
        supervisor.run(Cancellation::default()).await;
    });
    // This fixture intentionally leaves calibration absent so browser tests can
    // inspect the waiting state and explicitly exercise the real run endpoint.
    Ok(())
}
async fn extract() -> Json<Value> {
    Json(
        json!({"choices":[{"message":{"content":json!({"memories":[{"content":"Synthetic team completed the local exercise.","kind":"fact","tags":[]}]}).to_string()}}]}),
    )
}
async fn embeddings(Json(body): Json<Value>) -> Json<Value> {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../../tests/fixtures/memory-quality/calibration.json"
    ))
    .unwrap();
    let documents = corpus["documents"].as_array().unwrap();
    let queries = corpus["queries"].as_array().unwrap();
    Json(
        json!({"data":body["input"].as_array().unwrap().iter().enumerate().map(|(index,input)|{
        let text=input.as_str().unwrap();let doc=documents.iter().position(|doc|text.to_lowercase().contains(&doc["content"].as_str().unwrap().to_lowercase()));
        let query=queries.iter().find(|query|query["query"].as_str()==Some(text));
        let coordinate=doc.or_else(||query.and_then(|query|query["relevant"][0].as_str()).and_then(|id|documents.iter().position(|doc|doc["id"].as_str()==Some(id)))).unwrap_or(if query.is_some(){63}else{60});
        let mut vector=vec![0.0_f32;64];vector[coordinate]=1.0;json!({"index":index,"embedding":vector})
    }).collect::<Vec<_>>()}),
    )
}
