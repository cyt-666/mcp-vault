//! Opt-in disposable Admin browser fixture. Never loaded by production server.
use axum::{Json, Router, routing::post};
use mcp_vault_auth::{AuthService, SecretString};
use mcp_vault_core::{VaultCore, VaultCoreRuntime};
use mcp_vault_domain::{Actor, SourcePlane, VaultContext, VaultPath};
use mcp_vault_memory::{ExtractionPolicy, MemoryService, RememberInput};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderMode,
    ProviderService, ProviderSettings,
};
use mcp_vault_server::workers::{
    Cancellation, WorkerConfig, WorkerSupervisor, memory_extract_job_handler,
    memory_overview_job_handler,
};
use mcp_vault_state::StateStore;
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
    providers
        .bind_model(Some(context), "embedding_memory", model.id, json!({}), None)
        .await?;
    // Seed synthetic vectors only for browser contract checks.
    let sources = state
        .memory_units()
        .list(context, &Default::default(), 100, 0)
        .await?
        .into_iter()
        .map(|memory| mcp_vault_providers::EmbeddingSourceRef {
            object_type: "memory_unit".into(),
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
            "memory.overview",
            memory_overview_job_handler(state.clone(), memory.clone()),
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
    // Generation and overview controls use the real worker handlers.
    Ok(())
}
async fn extract(Json(request): Json<Value>) -> Json<Value> {
    let input: Value =
        serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
    let value = if request["response_format"]["json_schema"]["name"] == "memory_overview" {
        json!({"sections":[{"label":"Local practices","description":"Read these source units for local conditions and procedures.","unit_ids":input["units"].as_array().unwrap().iter().map(|unit|unit["id"].clone()).collect::<Vec<_>>()}]})
    } else {
        json!({"selections":input["units"].as_array().unwrap().iter().map(|unit|json!({"unit_id":unit["unit_id"],"kind":"experience","retrieval_hint":"local exercise"})).collect::<Vec<_>>()})
    };
    Json(json!({"choices":[{"message":{"content":value.to_string()}}]}))
}
async fn embeddings(Json(body): Json<Value>) -> Json<Value> {
    Json(
        json!({"data":body["input"].as_array().unwrap().iter().enumerate().map(|(index,input)|{
        let mut vector=vec![0.0_f32;64];
        for byte in input.as_str().unwrap().bytes() { vector[byte as usize % 64]+=1.0; }
        json!({"index":index,"embedding":vector})
    }).collect::<Vec<_>>()}),
    )
}
