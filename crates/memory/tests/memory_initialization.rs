use mcp_vault_auth::{AuthService, MasterKeyRing, SecretString};
use mcp_vault_core::{CommitPhase, FailureInjector, VaultCore, VaultCoreRuntime};
use mcp_vault_domain::{
    Actor, MemoryId, Revision, SourcePlane, VaultContext, VaultId, VaultPath, VaultSlug,
};
use mcp_vault_memory::{MemoryService, RememberInput, initialize_vault_memories};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderService,
    ProviderSettings,
};
use mcp_vault_state::{StateStore, VaultStatus};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::io::AsyncReadExt;

#[path = "../../state/tests/support/memory_v3_cutover.rs"]
mod snapshot;

struct FailAfterDelete(AtomicBool);
impl FailureInjector for FailAfterDelete {
    fn fail(&self, phase: CommitPhase) -> Result<(), &'static str> {
        if phase == CommitPhase::RenameCommitted && !self.0.swap(true, Ordering::SeqCst) {
            Err("test_after_delete_before_metadata")
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn v27_cutover_recovers_interruption_preserves_non_memory_data_and_never_clears_new_units() {
    let dir = tempfile::tempdir().unwrap();
    let database = format!("sqlite://{}", dir.path().join("state.sqlite3").display());
    snapshot::create_v27(&database).await;
    let state = StateStore::connect(&database).await.unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("main").unwrap(),
        dir.path().join("main"),
        Revision::ZERO,
    )
    .unwrap();
    let other = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("other").unwrap(),
        dir.path().join("other"),
        Revision::ZERO,
    )
    .unwrap();
    for context in [&context, &other] {
        state
            .vaults()
            .insert(context, "preserved Vault name", VaultStatus::Active)
            .await
            .unwrap();
    }
    let runtime = VaultCoreRuntime::default();
    let permit = runtime.maintenance_recovery_permit();
    let core = VaultCore::new(
        state.clone(),
        dir.path().join("history"),
        Default::default(),
        Default::default(),
        runtime.clone(),
    );
    let note = VaultPath::parse("ordinary.md").unwrap();
    let attachment = VaultPath::parse("attachment.bin").unwrap();
    let note_body = b"# Ordinary knowledge\nThis source survives the cutover verbatim.\n";
    for context in [&context, &other] {
        core.create_bytes(
            context,
            &note,
            note_body,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        core.create_bytes(
            context,
            &attachment,
            &[0, 255, 4, 8],
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    }
    let legacy_id = MemoryId::new();
    let legacy_path = core
        .managed_root()
        .join(&VaultPath::parse(&format!("memory/current/explicit/{legacy_id}.md")).unwrap())
        .unwrap();
    for context in [&context, &other] {
        let old = core
            .create_managed_bytes(
                context,
                &legacy_path,
                b"old managed body\n",
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await
            .unwrap();
        snapshot::seed_legacy(
            &database,
            &context.id().to_string(),
            &MemoryId::new().to_string(),
            &old.file.id.to_string(),
            legacy_path.as_str(),
        )
        .await;
        state
            .jobs()
            .enqueue(
                context,
                "memory.organize",
                &format!("old:{}", context.id()),
                &json!({"memory_contract_generation":3}),
                1,
                5,
                0,
            )
            .await
            .unwrap();
    }
    // A known predecessor namespace can contain an interrupted unregistered file.
    let orphan = context
        .content_root()
        .join(core.managed_root().as_str())
        .join("memory/orphan.md");
    std::fs::write(&orphan, b"orphan old memory\n").unwrap();
    let keep_managed = core
        .managed_root()
        .join(&VaultPath::parse("unrelated/config.md").unwrap())
        .unwrap();
    core.create_managed_bytes(
        &context,
        &keep_managed,
        b"unrelated managed content",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[23u8; 32]).unwrap(),
    );
    auth.setup_admin("operator", &SecretString::new("fixture-password-123"))
        .await
        .unwrap();
    let providers = ProviderService::new(state.clone(), auth);
    let provider = providers
        .create_provider(ProviderInput {
            name: "preserved provider".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: url::Url::parse("http://127.0.0.1:12000/v1/").unwrap(),
            settings: ProviderSettings::default(),
            enabled: true,
            secret: Some(SecretString::new("fixture-provider-secret")),
        })
        .await
        .unwrap();
    let model = providers
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "preserved-model".into(),
            capabilities: ModelCapabilities {
                structured_output: true,
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
    state
        .vaults()
        .set_status(&other, VaultStatus::Disabled)
        .await
        .unwrap();
    state.close().await;

    let _lock = StateStore::acquire_process_lock(&database).unwrap();
    let state = StateStore::connect_offline_exclusive(&database)
        .await
        .unwrap();
    state.migrate().await.unwrap();
    assert!(
        state
            .memory_units()
            .initialization_required(&context)
            .await
            .unwrap()
    );
    let core = VaultCore::new(
        state.clone(),
        dir.path().join("history"),
        Default::default(),
        Default::default(),
        runtime.clone(),
    );
    let interrupted = core
        .clone()
        .with_failure_injector(Arc::new(FailAfterDelete(AtomicBool::new(false))));
    assert!(
        initialize_vault_memories(&state, &context, &interrupted, &permit)
            .await
            .is_err()
    );
    assert_eq!(
        state
            .memory_units()
            .initialization(&context)
            .await
            .unwrap()
            .unwrap()
            .phase,
        "clearing"
    );
    let report = initialize_vault_memories(&state, &context, &core, &permit)
        .await
        .unwrap();
    assert_eq!(report.outcome, "initialized");
    assert_eq!(report.file_count, 2);
    assert!(
        state
            .memory_units()
            .legacy_counts(&context)
            .await
            .unwrap()
            .values()
            .all(|count| *count == 0)
    );
    assert_eq!(
        state.memory_units().legacy_counts(&other).await.unwrap()["memories"],
        1
    );
    assert!(
        state
            .memory_units()
            .initialization_required(&other)
            .await
            .unwrap()
    );
    for context in [&context, &other] {
        let mut read = core.read(context, &note).await.unwrap();
        let mut body = Vec::new();
        read.reader.read_to_end(&mut body).await.unwrap();
        assert_eq!(body, note_body);
        let mut read = core.read(context, &attachment).await.unwrap();
        let mut body = Vec::new();
        read.reader.read_to_end(&mut body).await.unwrap();
        assert_eq!(body, [0, 255, 4, 8]);
    }
    assert!(core.read_managed(&context, &legacy_path).await.is_err());
    assert!(!orphan.exists());
    assert!(core.read_managed(&context, &keep_managed).await.is_ok());
    assert!(
        !core
            .history(&context, &legacy_path)
            .await
            .unwrap()
            .is_empty()
    );
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[23u8; 32]).unwrap(),
    );
    assert!(
        state
            .auth()
            .find_admin_user_by_username("operator")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        auth.login_admin(
            "operator",
            &SecretString::new("fixture-password-123"),
            None,
            None
        )
        .await
        .is_ok()
    );
    assert_eq!(
        state
            .providers()
            .get_provider(provider.id)
            .await
            .unwrap()
            .unwrap()
            .secret_id,
        provider.secret_id
    );
    assert_eq!(
        state
            .providers()
            .resolve_binding(&context, "memory_extraction")
            .await
            .unwrap()
            .unwrap()
            .model_id,
        model.id
    );
    assert_eq!(
        state
            .providers()
            .get_provider(provider.id)
            .await
            .unwrap()
            .unwrap()
            .name,
        "preserved provider"
    );
    let service = MemoryService::new(state.clone(), auth);
    let current = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: "New exact memory after initialization.\n".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    let again = initialize_vault_memories(&state, &context, &core, &permit)
        .await
        .unwrap();
    assert_eq!(again.outcome, "already_initialized");
    assert_eq!(
        service.get(&context, current.id).await.unwrap().content,
        current.content
    );
    assert_eq!(
        initialize_vault_memories(&state, &other, &core, &permit)
            .await
            .unwrap()
            .outcome,
        "initialized"
    );
    assert_eq!(
        state
            .vaults()
            .find_by_id(other.id())
            .await
            .unwrap()
            .unwrap()
            .status,
        VaultStatus::Disabled
    );
    assert!(state.integrity_check().await.unwrap().integrity_ok);
    state.close().await;
}
