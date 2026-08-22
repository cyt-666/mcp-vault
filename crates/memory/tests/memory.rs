use axum::{Json, Router, routing::post};
use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_core::VaultCore;
use mcp_vault_domain::{
    Actor, Revision, SourcePlane, VaultContext, VaultId, VaultPath, VaultSlug, WritePrecondition,
};
use mcp_vault_memory::{
    MemoryOrigin, MemoryService, MemorySourceInput, MemoryStatus, MemoryType, MemoryUpdateInput,
    RecallContext, RecallRequest, RememberInput,
};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ProviderInput, ProviderKind, ProviderMode, ProviderService,
    ProviderSettings,
};
use mcp_vault_state::{StateStore, VaultStatus};
use mcp_vault_storage_fs::StorageOptions;
use serde_json::json;
use tempfile::TempDir;
use tokio::io::AsyncReadExt;

async fn fake_extraction() -> Json<serde_json::Value> {
    Json(json!({
        "choices": [{
            "message": {
                "content": "{\"memories\":[{\"type\":\"decision\",\"content\":\"The service uses a durable extraction candidate.\",\"importance\":0.9,\"confidence\":0.97,\"entities\":[\"MCP Vault\"],\"tags\":[\"memory\"],\"source_anchor\":{\"heading\":[],\"start_line\":1,\"end_line\":1}}]}"
            }
        }]
    }))
}

async fn fixture(
    state: &StateStore,
    directory: &TempDir,
    slug: &str,
) -> (VaultContext, VaultCore, MemoryService) {
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new(slug).unwrap(),
        directory.path().join(slug),
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
        MasterKeyRing::from_bytes(1, &[7_u8; 32]).unwrap(),
    );
    (context, core, MemoryService::new(state.clone(), auth))
}

#[tokio::test]
async fn remember_is_idempotent_materialized_and_recalled_lexically() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (context, core, service) = fixture(&state, &directory, "work").await;
    let input = RememberInput {
        content: "The service preserves WebDAV revisions.\n".to_owned(),
        memory_type: MemoryType::Decision,
        importance: 0.95,
        confidence: 0.99,
        valid_from: None,
        valid_to: None,
        tags: vec!["architecture".to_owned()],
        entities: vec!["WebDAV".to_owned()],
        sources: Vec::new(),
        supersedes: None,
        idempotency_key: Some("remember-1".to_owned()),
        origin: MemoryOrigin::ExplicitAgent,
        extraction: json!({"prompt_version": "manual"}),
    };
    let created = service
        .remember(&context, &core, input.clone())
        .await
        .unwrap();
    assert_eq!(created.outcome, "created");
    assert_eq!(created.memory.status, MemoryStatus::Active);
    assert!(created.memory.canonical_path.is_some());

    let path = created.memory.canonical_path.clone().unwrap();
    let managed = core.read_managed(&context, &path).await.unwrap();
    let mut reader = managed.reader;
    let mut body = String::new();
    reader.read_to_string(&mut body).await.unwrap();
    assert!(body.contains("The service preserves WebDAV revisions."));
    assert!(body.contains("status: \"active\""));

    let repeated = service.remember(&context, &core, input).await.unwrap();
    assert_eq!(repeated.outcome, "created");
    assert_eq!(repeated.memory.id, created.memory.id);

    let recalled = service
        .recall(
            &context,
            RecallRequest {
                query: "WebDAV revisions".to_owned(),
                context: RecallContext::default(),
                max_results: 10,
                max_tokens: 500,
                ..RecallRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(recalled.memories.len(), 1);
    assert_eq!(recalled.memories[0].id, created.memory.id);
    assert!(recalled.memories[0].sources.len() == 1);

    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[7_u8; 32]).unwrap(),
    );
    let providers = ProviderService::new(state.clone(), auth);
    providers
        .set_provider_mode(&context, ProviderMode::LocalOnly, None)
        .await
        .unwrap();
    let provider = providers
        .create_provider(ProviderInput {
            name: "unavailable-local".to_owned(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: url::Url::parse("http://127.0.0.1:9/v1/").unwrap(),
            settings: ProviderSettings {
                timeout_ms: 100,
                connect_timeout_ms: 50,
                max_retries: 0,
                ..ProviderSettings::default()
            },
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();
    let model = providers
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "unavailable-embedding".to_owned(),
            capabilities: ModelCapabilities {
                embeddings: true,
                dimension: Some(3),
                ..ModelCapabilities::default()
            },
            settings: json!({}),
            enabled: true,
        })
        .await
        .unwrap();
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
    let degraded = service
        .recall(
            &context,
            RecallRequest {
                query: "WebDAV revisions".to_owned(),
                max_results: 10,
                max_tokens: 500,
                ..RecallRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(degraded.memories.len(), 1);
    assert!(
        degraded
            .degraded
            .iter()
            .any(|reason| reason == "semantic_provider_unavailable")
    );
}

#[tokio::test]
async fn memory_updates_archive_and_vault_isolation_are_revision_aware() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (first, core, service) = fixture(&state, &directory, "first").await;
    let (second, second_core, second_service) = fixture(&state, &directory, "second").await;
    let created = service
        .remember(
            &first,
            &core,
            RememberInput {
                content: "The current project is MCP Vault.".to_owned(),
                memory_type: MemoryType::Project,
                importance: 0.8,
                confidence: 0.9,
                valid_from: None,
                valid_to: None,
                tags: Vec::new(),
                entities: vec!["MCP Vault".to_owned()],
                sources: Vec::new(),
                supersedes: None,
                idempotency_key: None,
                origin: MemoryOrigin::ExplicitAgent,
                extraction: json!({}),
            },
        )
        .await
        .unwrap();
    let updated = service
        .update(
            &first,
            &core,
            created.memory.id,
            created.memory.revision,
            MemoryUpdateInput {
                content: Some("The current project is MCP Vault WP-11.".to_owned()),
                ..MemoryUpdateInput::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        updated.revision.value(),
        created.memory.revision.value() + 1
    );
    let archived = service
        .forget(&first, &core, created.memory.id, updated.revision, false)
        .await
        .unwrap();
    assert_eq!(archived.status, MemoryStatus::Archived);

    let other_recall = second_service
        .recall(
            &second,
            RecallRequest {
                query: "MCP Vault".to_owned(),
                max_results: 10,
                max_tokens: 500,
                ..RecallRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(other_recall.memories.is_empty());
    assert!(
        second_core
            .read_managed(
                &second,
                &VaultPath::parse(
                    "_mcp-vault/memory/records/2099/01/00000000-0000-7000-8000-000000000000.md"
                )
                .unwrap(),
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn extracted_memory_source_invalidation_marks_only_unsupported_memory_stale() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (context, core, service) = fixture(&state, &directory, "source").await;
    let source = core
        .create_bytes(
            &context,
            &VaultPath::parse("notes/source.md").unwrap(),
            b"source",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let extracted = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: "The source supports this fact.".to_owned(),
                memory_type: MemoryType::Fact,
                importance: 0.8,
                confidence: 0.95,
                valid_from: None,
                valid_to: None,
                tags: Vec::new(),
                entities: Vec::new(),
                sources: vec![MemorySourceInput {
                    source_type: "note".to_owned(),
                    note_file_id: Some(source.file.id),
                    note_path: Some(source.file.path.clone()),
                    note_revision: Some(source.file.current_revision),
                    ..MemorySourceInput::default()
                }],
                supersedes: None,
                idempotency_key: None,
                origin: MemoryOrigin::Extracted,
                extraction: json!({"pipeline_version": 1}),
            },
        )
        .await
        .unwrap();
    let stale = service
        .invalidate_source(&context, &core, source.file.id, true)
        .await
        .unwrap();
    assert_eq!(stale, 1);
    assert_eq!(
        service
            .get(&context, extracted.memory.id)
            .await
            .unwrap()
            .status,
        MemoryStatus::Stale
    );
}

#[tokio::test]
async fn changed_source_stales_then_reactivates_same_memory_with_current_provenance() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (context, core, service) = fixture(&state, &directory, "source-update").await;
    let path = VaultPath::parse("notes/source.md").unwrap();
    let source = core
        .create_bytes(
            &context,
            &path,
            b"the original source",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let input = |revision| RememberInput {
        content: "The source supports a durable fact.".to_owned(),
        memory_type: MemoryType::Fact,
        importance: 0.8,
        confidence: 0.95,
        sources: vec![MemorySourceInput {
            source_type: "note".to_owned(),
            note_file_id: Some(source.file.id),
            note_path: Some(path.clone()),
            note_revision: Some(revision),
            ..MemorySourceInput::default()
        }],
        origin: MemoryOrigin::Extracted,
        ..RememberInput::default()
    };
    let created = service
        .remember(&context, &core, input(source.file.current_revision))
        .await
        .unwrap();
    let changed = core
        .replace_bytes(
            &context,
            &path,
            source.file.current_revision,
            b"the revised source",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .invalidate_source(&context, &core, source.file.id, false)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        service
            .get(&context, created.memory.id)
            .await
            .unwrap()
            .status,
        MemoryStatus::Stale
    );

    let revived = service
        .remember(&context, &core, input(changed.file.current_revision))
        .await
        .unwrap();
    assert_eq!(revived.memory.id, created.memory.id);
    assert_eq!(revived.memory.status, MemoryStatus::Active);
    let sources = service
        .get(&context, created.memory.id)
        .await
        .unwrap()
        .sources;
    assert!(sources.iter().any(|source| {
        source.file_id == Some(changed.file.id)
            && source.revision == Some(changed.file.current_revision)
    }));
}

#[tokio::test]
async fn deleted_source_keeps_memory_when_a_second_current_note_supports_it() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (context, core, service) = fixture(&state, &directory, "source-support").await;
    let first_path = VaultPath::parse("notes/first.md").unwrap();
    let second_path = VaultPath::parse("notes/second.md").unwrap();
    let first = core
        .create_bytes(
            &context,
            &first_path,
            b"first source",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let second = core
        .create_bytes(
            &context,
            &second_path,
            b"second source",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let memory = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: "Two notes support this extracted fact.".to_owned(),
                memory_type: MemoryType::Fact,
                importance: 0.8,
                confidence: 0.95,
                sources: vec![
                    MemorySourceInput {
                        source_type: "note".to_owned(),
                        note_file_id: Some(first.file.id),
                        note_path: Some(first_path),
                        note_revision: Some(first.file.current_revision),
                        ..MemorySourceInput::default()
                    },
                    MemorySourceInput {
                        source_type: "note".to_owned(),
                        note_file_id: Some(second.file.id),
                        note_path: Some(second_path),
                        note_revision: Some(second.file.current_revision),
                        ..MemorySourceInput::default()
                    },
                ],
                origin: MemoryOrigin::Extracted,
                ..RememberInput::default()
            },
        )
        .await
        .unwrap();
    core.delete(
        &context,
        &first.file.path,
        first.file.current_revision,
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        service
            .invalidate_source(&context, &core, first.file.id, true)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        service
            .get(&context, memory.memory.id)
            .await
            .unwrap()
            .status,
        MemoryStatus::Active
    );
}

#[tokio::test]
async fn explicit_newer_memory_supersedes_old_and_historical_recall_keeps_both() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (context, core, service) = fixture(&state, &directory, "supersession").await;
    let old = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: "The backend uses Go.".to_owned(),
                memory_type: MemoryType::Decision,
                importance: 0.8,
                confidence: 0.8,
                ..RememberInput::default()
            },
        )
        .await
        .unwrap();
    let newer = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: "The backend uses Rust.".to_owned(),
                memory_type: MemoryType::Decision,
                importance: 0.95,
                confidence: 0.99,
                supersedes: Some(old.memory.id),
                ..RememberInput::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        service.get(&context, old.memory.id).await.unwrap().status,
        MemoryStatus::Superseded
    );
    assert_eq!(newer.memory.status, MemoryStatus::Active);
    assert_eq!(newer.memory.relations.len(), 1);
    assert_eq!(newer.memory.relations[0].memory_id, old.memory.id);

    let current = service
        .recall(
            &context,
            RecallRequest {
                query: "backend uses".to_owned(),
                max_results: 10,
                max_tokens: 500,
                ..RecallRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(current.memories.len(), 1);
    assert_eq!(current.memories[0].id, newer.memory.id);

    let historical = service
        .recall(
            &context,
            RecallRequest {
                query: "backend uses".to_owned(),
                include_historical: true,
                max_results: 10,
                max_tokens: 500,
                ..RecallRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(
        historical
            .memories
            .iter()
            .any(|memory| memory.id == old.memory.id)
    );
    assert!(
        historical
            .memories
            .iter()
            .any(|memory| memory.id == newer.memory.id)
    );

    let report = service.rebuild(&context, &core).await.unwrap();
    assert_eq!(report.quarantined, 0);
    assert_eq!(
        service
            .get(&context, newer.memory.id)
            .await
            .unwrap()
            .relations
            .len(),
        1
    );
}

#[tokio::test]
async fn invalid_managed_markdown_is_quarantined_without_becoming_recallable() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (context, core, service) = fixture(&state, &directory, "quarantine").await;
    let created = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: "A valid durable proposition.".to_owned(),
                memory_type: MemoryType::Fact,
                importance: 0.8,
                confidence: 0.9,
                ..RememberInput::default()
            },
        )
        .await
        .unwrap();
    let path = created.memory.canonical_path.unwrap();
    core.replace_managed_bytes(
        &context,
        &path,
        created.memory.canonical_revision.unwrap(),
        b"not frontmatter",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let report = service.rebuild(&context, &core).await.unwrap();
    assert_eq!(report.quarantined, 1);
    let recall = service
        .recall(
            &context,
            RecallRequest {
                query: "valid durable proposition".to_owned(),
                max_results: 10,
                max_tokens: 500,
                ..RecallRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(recall.memories.is_empty());
}

#[tokio::test]
async fn missing_managed_memory_file_is_quarantined_during_rebuild() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (context, core, service) = fixture(&state, &directory, "missing-memory").await;
    let created = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: "A canonical file must remain recoverable.".to_owned(),
                memory_type: MemoryType::Constraint,
                importance: 0.9,
                confidence: 0.95,
                ..RememberInput::default()
            },
        )
        .await
        .unwrap();
    core.delete_managed(
        &context,
        &created.memory.canonical_path.clone().unwrap(),
        created.memory.canonical_revision.unwrap(),
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let report = service.rebuild(&context, &core).await.unwrap();
    assert_eq!(report.quarantined, 1);
    assert_eq!(
        service
            .get(&context, created.memory.id)
            .await
            .unwrap()
            .status,
        MemoryStatus::Quarantined
    );
}

#[tokio::test]
async fn extraction_creates_reviewable_candidate_and_promotion_rechecks_source() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (context, core, service) = fixture(&state, &directory, "extract").await;
    let note = core
        .create_bytes(
            &context,
            &VaultPath::parse("notes/extract.md").unwrap(),
            b"A durable decision is documented here.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/v1/chat/completions", post(fake_extraction)),
        )
        .await
        .unwrap();
    });
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[7_u8; 32]).unwrap(),
    );
    let providers = ProviderService::new(state.clone(), auth);
    providers
        .set_provider_mode(&context, ProviderMode::LocalOnly, None)
        .await
        .unwrap();
    let provider = providers
        .create_provider(ProviderInput {
            name: "fake-extraction".to_owned(),
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
            external_model_id: "fake-extractor".to_owned(),
            capabilities: ModelCapabilities {
                structured_output: true,
                ..ModelCapabilities::default()
            },
            settings: json!({}),
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
        .settings()
        .set_vault(
            &context,
            "memory.extraction.policy",
            &json!({"auto_promote": false}),
            WritePrecondition::Unconditional,
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        service
            .extract_note(&context, &core, &note.file.path)
            .await
            .unwrap(),
        1
    );
    let candidates = service
        .list_candidates(&context, None, 10, 0)
        .await
        .unwrap();
    assert_eq!(candidates.len(), 1);
    assert!(
        service
            .list(
                &context,
                vec![MemoryStatus::Active],
                Vec::new(),
                None,
                None,
                None,
                10,
                0
            )
            .await
            .unwrap()
            .is_empty()
    );

    let promoted = service
        .promote_candidate(&context, &core, candidates[0].id)
        .await
        .unwrap();
    assert_eq!(promoted.memory.status, MemoryStatus::Active);
    server.abort();
}
