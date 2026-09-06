//! Metadata edits preserve valid vector identity and optimistic concurrency.
use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderService,
    ProviderSettings,
};
use mcp_vault_state::StateStore;

#[tokio::test]
async fn renaming_provider_should_preserve_valid_embedding_identity() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[4; 32]).unwrap(),
    );
    let service = ProviderService::new(state.clone(), auth);
    let mut input = ProviderInput {
        name: "before".into(),
        kind: ProviderKind::EmbeddingHttp,
        base_url: "https://example.invalid/v1/".parse().unwrap(),
        settings: ProviderSettings::default(),
        enabled: true,
        secret: None,
    };
    let provider = service.create_provider(input.clone()).await.unwrap();
    let model = service
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "embedding-test".into(),
            capabilities: ModelCapabilities {
                embeddings: true,
                dimension: Some(2),
                ..Default::default()
            },
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await
        .unwrap();
    let before = service.embeddings().profile_hash(model.id).await.unwrap();
    input.name = "after".into();
    let stale = provider.clone();
    let renamed = service
        .update_provider(provider, input.clone())
        .await
        .unwrap();
    let after = service.embeddings().profile_hash(model.id).await.unwrap();
    assert_eq!(
        before, after,
        "display-only Provider rename must not invalidate valid business vectors"
    );
    assert_ne!(renamed.revision, stale.revision, "Admin CAS still advances");
    assert!(
        service.update_provider(stale, input.clone()).await.is_err(),
        "stale Admin updates must still conflict"
    );
    let unchanged_model = state.providers().update_model(&model).await.unwrap();
    assert_ne!(unchanged_model.revision, model.revision);
    assert_eq!(
        before,
        service.embeddings().profile_hash(model.id).await.unwrap(),
        "discovery of unchanged capabilities must preserve vectors"
    );
    input.enabled = false;
    let disabled = service
        .update_provider(renamed, input.clone())
        .await
        .unwrap();
    assert_eq!(
        before,
        service.embeddings().profile_hash(model.id).await.unwrap()
    );
    input.enabled = true;
    input.base_url = "https://changed.example.invalid/v1/".parse().unwrap();
    service.update_provider(disabled, input).await.unwrap();
    let changed = service.embeddings().profile_hash(model.id).await.unwrap();
    assert_ne!(before, changed, "real endpoint changes invalidate vectors");
    let mut changed_model = unchanged_model;
    changed_model.capabilities["dimension"] = serde_json::json!(3);
    state
        .providers()
        .update_model(&changed_model)
        .await
        .unwrap();
    assert_ne!(
        changed,
        service.embeddings().profile_hash(model.id).await.unwrap(),
        "dimension changes invalidate vectors"
    );
}
