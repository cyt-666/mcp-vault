//! Example-only local contract Provider; never linked into the service.
use axum::{Json, Router, routing::post};
use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_domain::{Revision, VaultContext, VaultId, VaultSlug};
use mcp_vault_memory::MemoryService;
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderMode,
    ProviderService, ProviderSettings,
};
use mcp_vault_state::{StateStore, VaultStatus};
use serde_json::{Value, json};

async fn embeddings(Json(request): Json<Value>) -> Json<Value> {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/memory-quality/calibration.json"
    ))
    .unwrap();
    let documents = corpus["documents"].as_array().unwrap();
    let queries = corpus["queries"].as_array().unwrap();
    Json(
        json!({"data":request["input"].as_array().unwrap().iter().enumerate().map(|(index,input)|{
        let text=input.as_str().unwrap();
        let document=documents.iter().position(|doc|text.to_lowercase().contains(&doc["content"].as_str().unwrap().to_lowercase()));
        let query=queries.iter().find(|query|query["query"].as_str()==Some(text));
        let coordinate=document.or_else(||query.and_then(|query|query["relevant"][0].as_str()).and_then(|id|documents.iter().position(|doc|doc["id"].as_str()==Some(id)))).unwrap_or(if query.is_some() {63} else {60});
        let mut vector=vec![0.0_f32;64]; vector[coordinate]=1.0;
        json!({"index":index,"embedding":vector})
    }).collect::<Vec<_>>()}),
    )
}

pub async fn evaluate() -> Result<Value, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let state = StateStore::connect_and_migrate("sqlite::memory:").await?;
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("local-calibration-contract")?,
        dir.path().join("unused"),
        Revision::ZERO,
    )?;
    state
        .vaults()
        .insert(&context, "Synthetic contract only", VaultStatus::Active)
        .await?;
    let providers = ProviderService::new(
        state.clone(),
        AuthService::new(state.auth(), MasterKeyRing::from_bytes(1, &[91; 32])?),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/v1/embeddings", post(embeddings)),
        )
        .await
        .unwrap();
    });
    providers
        .set_provider_mode(&context, ProviderMode::LocalOnly, None)
        .await?;
    let provider = providers
        .create_provider(ProviderInput {
            name: "example-only synthetic contract".into(),
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
            external_model_id: "synthetic-contract-not-quality-evidence".into(),
            capabilities: ModelCapabilities {
                embeddings: true,
                dimension: Some(64),
                ..Default::default()
            },
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await?;
    let memory = MemoryService::with_provider_service(state, providers.clone());
    let mut reports = Vec::new();
    for channel in ["memory", "note"] {
        providers
            .bind_model(
                Some(&context),
                &format!("embedding_{channel}"),
                model.id,
                json!({}),
                None,
            )
            .await?;
        let profile = memory
            .calibration_profile(&context, channel)
            .await?
            .ok_or("contract profile missing")?;
        let report = memory
            .execute_retrieval_calibration(&context, &profile)
            .await?;
        if !report.passed {
            return Err(format!(
                "synthetic contract calibration did not pass the real engine: {}",
                serde_json::to_string(&report)?
            )
            .into());
        }
        reports.push(report);
    }
    server.abort();
    Ok(
        json!({"execution_provenance":"local_synthetic_contract","semantic_quality_proven":false,"engine":"MemoryService::execute_retrieval_calibration","channels":reports}),
    )
}
