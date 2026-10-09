use std::path::PathBuf;

use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_core::VaultCore;
use mcp_vault_domain::{
    Actor, ActorId, ActorType, Revision, SourcePlane, VaultContext, VaultId, VaultPath,
    VaultPathPolicy, VaultSlug, WritePrecondition,
};
use mcp_vault_memory::{
    MemoryPackRequest, MemoryService, SemanticExplicitDeleteRequest, SemanticExplicitFacade,
    SemanticExplicitListRequest, SemanticExplicitSourceRequest, SemanticExplicitUpdatePatch,
    SemanticExplicitUpdateRequest, SemanticMemoryService, SemanticRememberExplicitRequest,
};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderError, ProviderInput, ProviderKind,
    ProviderMode, ProviderService, ProviderSettings, new_embedding_id,
};
use mcp_vault_state::{EmbeddingRecord, StateStore, VaultStatus};
use mcp_vault_storage_fs::{DurabilityPolicy, StorageOptions};

async fn fixture() -> (
    tempfile::TempDir,
    String,
    StateStore,
    VaultContext,
    VaultCore,
    MemoryService,
) {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("semantic-explicit.sqlite3");
    let url = format!("sqlite://{}", database.display());
    let state = StateStore::connect_and_migrate(&url).await.unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("explicit").unwrap(),
        PathBuf::from(dir.path()).join("vault"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&context, "explicit", VaultStatus::Active)
        .await
        .unwrap();
    state
        .settings()
        .set_vault(
            &context,
            "memory.units.policy",
            &serde_json::json!({"enabled":true,"request_timeout_seconds":300}),
            WritePrecondition::Unconditional,
            None,
        )
        .await
        .unwrap();
    let core = VaultCore::new(
        state.clone(),
        dir.path().join("history"),
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
        Default::default(),
    );
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[29u8; 32]).unwrap(),
    );
    let memory = MemoryService::new(state.clone(), auth);
    (dir, url, state, context, core, memory)
}

#[tokio::test]
async fn explicit_facade_preserves_ownership_idempotency_restart_and_no_binding_jobs() {
    let (_dir, url, state, context, core, memory) = fixture().await;
    let source_path = VaultPath::parse("notes/source.md").unwrap();
    let source_file = core
        .create_bytes(
            &context,
            &source_path,
            b"source reference\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    let facade = SemanticExplicitFacade::new(memory.clone());
    let actor = || Actor::identified(ActorType::McpPat, ActorId::new("agent-explicit").unwrap());
    let request = SemanticRememberExplicitRequest {
        content: "  Keep this exact explicit assertion.\r\n".into(),
        memory_type: None,
        importance: None,
        confidence: None,
        valid_from: None,
        valid_to: None,
        tags: vec!["explicit".into()],
        entities: Vec::new(),
        sources: vec![SemanticExplicitSourceRequest {
            path: source_path.to_string(),
            file_id: source_file.id.to_string(),
            revision: source_file.current_revision.value(),
            heading: Vec::new(),
            start_line: None,
            end_line: None,
            excerpt_hash: None,
        }],
        idempotency_key: "explicit-v1-once".into(),
    };
    let first = facade
        .remember(
            &context,
            &core,
            actor(),
            SourcePlane::Mcp,
            mcp_vault_memory::MemoryOrigin::ExplicitAgent,
            request.clone(),
        )
        .await
        .unwrap();
    assert_eq!(first.memory.ownership, "explicit");
    assert_eq!(first.memory.content, request.content);
    assert!(first.memory.embedding_eligible);
    assert!(!first.memory.embedding_binding_present);
    assert!(first.memory.canonical_file_id.is_some());
    assert!(
        state
            .jobs()
            .list(&context, None, Some("embedding.rebuild"), 200, 0)
            .await
            .unwrap()
            .is_empty()
    );
    let retry = facade
        .remember(
            &context,
            &core,
            actor(),
            SourcePlane::Mcp,
            mcp_vault_memory::MemoryOrigin::ExplicitAgent,
            request.clone(),
        )
        .await
        .unwrap();
    assert_eq!(retry.outcome, "stored_existing");
    assert_eq!(retry.memory.memory_id, first.memory.memory_id);
    let mut different = request.clone();
    different.content.push_str("different");
    assert!(
        facade
            .remember(
                &context,
                &core,
                actor(),
                SourcePlane::Mcp,
                mcp_vault_memory::MemoryOrigin::ExplicitAgent,
                different,
            )
            .await
            .is_err()
    );

    let bad_source = SemanticRememberExplicitRequest {
        idempotency_key: "explicit-v1-bad-source".into(),
        sources: vec![SemanticExplicitSourceRequest {
            path: source_path.to_string(),
            file_id: source_file.id.to_string(),
            revision: source_file.current_revision.value() + 1,
            heading: Vec::new(),
            start_line: None,
            end_line: None,
            excerpt_hash: None,
        }],
        ..request.clone()
    };
    assert!(
        facade
            .remember(
                &context,
                &core,
                actor(),
                SourcePlane::Mcp,
                mcp_vault_memory::MemoryOrigin::ExplicitAgent,
                bad_source,
            )
            .await
            .is_err()
    );

    core.delete(
        &context,
        &source_path,
        source_file.current_revision,
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    core.create_bytes(
        &context,
        &source_path,
        b"recreated source\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let stale_recreated = SemanticRememberExplicitRequest {
        idempotency_key: "explicit-v1-stale-recreated".into(),
        ..request.clone()
    };
    assert!(
        facade
            .remember(
                &context,
                &core,
                actor(),
                SourcePlane::Mcp,
                mcp_vault_memory::MemoryOrigin::ExplicitAgent,
                stale_recreated,
            )
            .await
            .is_err()
    );

    assert!(
        SemanticMemoryService::new(state.clone())
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .is_empty()
    );
    let pack = mcp_vault_memory::MemoryPackService::new(state.clone())
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "explicit assertion".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(pack.current_context.is_empty() && pack.relevant_experiences.is_empty());
    let canonical_path = VaultPath::parse(&first.memory.canonical_path.clone().unwrap()).unwrap();
    assert!(core.read_managed(&context, &canonical_path).await.is_ok());

    drop(core);
    drop(state);
    let reopened = StateStore::connect_and_migrate(&url).await.unwrap();
    let reopened_context = context.clone();
    let reopened_core = VaultCore::new(
        reopened.clone(),
        _dir.path().join("history"),
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
        Default::default(),
    );
    let reopened_memory = MemoryService::new(
        reopened.clone(),
        AuthService::new(
            reopened.auth(),
            MasterKeyRing::from_bytes(1, &[29u8; 32]).unwrap(),
        ),
    );
    let loaded = reopened_memory
        .get(&reopened_context, first.memory.memory_id.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(loaded.content, request.content);
    let foreign_context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("foreign").unwrap(),
        _dir.path().join("foreign-vault"),
        Revision::ZERO,
    )
    .unwrap();
    reopened
        .vaults()
        .insert(&foreign_context, "foreign", VaultStatus::Active)
        .await
        .unwrap();
    assert!(
        reopened_memory
            .get(&foreign_context, first.memory.memory_id.parse().unwrap())
            .await
            .is_err()
    );
    assert!(
        reopened_core
            .read_managed(&reopened_context, &canonical_path)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn explicit_facade_enqueues_existing_embedding_job_when_binding_is_present() {
    let (_dir, _url, state, context, core, _memory) = fixture().await;
    let providers = ProviderService::new(
        state.clone(),
        AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[31u8; 32]).unwrap(),
        ),
    );
    let provider = providers
        .create_provider(ProviderInput {
            name: "embedding fixture".into(),
            kind: ProviderKind::EmbeddingHttp,
            base_url: url::Url::parse("http://127.0.0.1:12000/v1/").unwrap(),
            settings: ProviderSettings::default(),
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();
    let model = providers
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "embedding-fixture".into(),
            capabilities: ModelCapabilities {
                embeddings: true,
                dimension: Some(3),
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
            "embedding_memory",
            model.id,
            serde_json::json!({}),
            None,
        )
        .await
        .unwrap();
    let facade = SemanticExplicitFacade::new(MemoryService::with_provider_service(
        state.clone(),
        providers,
    ));
    let result = facade
        .remember(
            &context,
            &core,
            Actor::identified(ActorType::McpPat, ActorId::new("embedding-agent").unwrap()),
            SourcePlane::Mcp,
            mcp_vault_memory::MemoryOrigin::ExplicitAgent,
            SemanticRememberExplicitRequest {
                content: "Embedding-eligible explicit assertion.".into(),
                idempotency_key: "embedding-explicit-v1".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(result.memory.embedding_binding_present);
    let jobs = state
        .jobs()
        .list(&context, None, Some("embedding.rebuild"), 200, 0)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
}

#[tokio::test]
async fn legacy_memory_embedding_binding_without_capability_is_blocked_and_preserved() {
    let (_dir, _url, state, context, core, _memory) = fixture().await;
    let providers = ProviderService::new(
        state.clone(),
        AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[47_u8; 32]).unwrap(),
        ),
    );
    providers
        .set_provider_mode(&context, ProviderMode::Enabled, None)
        .await
        .unwrap();
    let provider = providers
        .create_provider(ProviderInput {
            name: "legacy embedding fixture".into(),
            kind: ProviderKind::EmbeddingHttp,
            base_url: url::Url::parse("http://127.0.0.1:12000/v1/").unwrap(),
            settings: ProviderSettings::default(),
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();
    let model = providers
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "legacy-no-embeddings".into(),
            capabilities: ModelCapabilities::default(),
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await
        .unwrap();
    let binding = state
        .providers()
        .upsert_binding(
            Some(&context),
            "embedding_memory",
            model.id,
            &serde_json::json!({}),
            None,
        )
        .await
        .unwrap();

    let memory = MemoryService::with_provider_service(state.clone(), providers);
    let facade = SemanticExplicitFacade::new(memory.clone());
    let result = facade
        .remember(
            &context,
            &core,
            Actor::identified(
                ActorType::McpPat,
                ActorId::new("legacy-embedding-agent").unwrap(),
            ),
            SourcePlane::Mcp,
            mcp_vault_memory::MemoryOrigin::ExplicitAgent,
            SemanticRememberExplicitRequest {
                content: "Legacy embedding capability fixture.".into(),
                idempotency_key: "legacy-embedding-v1".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!result.memory.embedding_binding_present);

    let memory_id = result.memory.memory_id.parse().unwrap();
    let unit = state
        .memory_units()
        .get(&context, memory_id)
        .await
        .unwrap()
        .unwrap()
        .memory;
    let old_vector = EmbeddingRecord {
        id: new_embedding_id(),
        vault_id: context.id(),
        object_type: "memory_unit".into(),
        object_id: unit.id.to_string(),
        chunk_key: "body-v3:0000".into(),
        provider_id: provider.id,
        model_id: model.id,
        dimension: 3,
        content_hash: unit.content_hash,
        profile_hash: "legacy-profile".into(),
        input_hash: "legacy-input".into(),
        vector_backend_key: format!("{}:memory_unit:{}:body-v3:0000", context.id(), unit.id),
        created_at: 1,
        updated_at: 1,
    };
    state
        .providers()
        .upsert_embedding(&context, &old_vector, &[1.0, 0.0, 0.0])
        .await
        .unwrap();

    let status = memory.embedding_status(&context).await.unwrap();
    assert!(status.configured);
    assert_eq!(status.profile_hash, None);
    assert_eq!(status.current, 0);
    assert!(
        status
            .blockers
            .contains(&"embedding_model_capability_unavailable".to_owned())
    );
    let schedule_error = memory
        .schedule_memory_embeddings(&context)
        .await
        .unwrap_err();
    assert!(matches!(
        schedule_error,
        mcp_vault_memory::MemoryError::Provider(ProviderError::ModelCapabilityMismatch {
            capability: "embeddings"
        })
    ));
    let jobs = state
        .jobs()
        .list(&context, None, Some("embedding.rebuild"), 200, 0)
        .await
        .unwrap();
    assert!(jobs.is_empty());
    let retained = state
        .providers()
        .list_embeddings(&context, model.id, "memory_unit", 10, 0)
        .await
        .unwrap();
    assert_eq!(retained, vec![old_vector]);
    assert_eq!(
        state
            .providers()
            .get_binding(Some(&context), "embedding_memory")
            .await
            .unwrap()
            .unwrap()
            .id,
        binding.id
    );
}

#[tokio::test]
async fn explicit_facade_raw_crud_is_explicit_only_revision_fenced_and_cursor_stable() {
    let (_dir, _url, state, context, core, memory) = fixture().await;
    let facade = SemanticExplicitFacade::new(memory);
    let actor = || Actor::identified(ActorType::McpPat, ActorId::new("crud-agent").unwrap());

    let mut ids = Vec::new();
    for (index, content) in [
        "raw CRUD memory one",
        "raw CRUD memory two",
        "raw CRUD memory three",
    ]
    .into_iter()
    .enumerate()
    {
        let result = facade
            .remember(
                &context,
                &core,
                actor(),
                SourcePlane::Mcp,
                mcp_vault_memory::MemoryOrigin::ExplicitAgent,
                SemanticRememberExplicitRequest {
                    content: content.to_owned(),
                    idempotency_key: format!("raw-crud-{index}"),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        ids.push(result.memory.memory_id);
    }

    let first_page = facade
        .list(
            &context,
            &SemanticExplicitListRequest {
                limit: Some(2),
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(first_page.memories.len(), 2);
    assert!(first_page.truncated);
    assert_eq!(
        first_page.next_cursor,
        Some(first_page.memories[1].memory_id.clone())
    );

    let second_page = facade
        .list(
            &context,
            &SemanticExplicitListRequest {
                limit: Some(2),
                cursor: first_page.next_cursor.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(second_page.memories.len(), 1);
    assert!(!second_page.truncated);
    assert!(second_page.next_cursor.is_none());
    assert!(
        first_page
            .memories
            .iter()
            .chain(second_page.memories.iter())
            .all(|memory| memory.ownership == "explicit")
    );

    let selected = facade.get(&context, &ids[0]).await.unwrap().unwrap();
    assert_eq!(selected.content, "raw CRUD memory one");
    assert_eq!(selected.revision, 1);
    let updated = facade
        .update(
            &context,
            &core,
            actor(),
            SourcePlane::Mcp,
            &SemanticExplicitUpdateRequest {
                memory_id: ids[0].clone(),
                expected_revision: selected.revision,
                patch: SemanticExplicitUpdatePatch {
                    content: Some("raw CRUD memory one updated".to_owned()),
                    tags: Some(vec!["legal-tag".to_owned()]),
                    entities: Some(vec!["legal-entity".to_owned()]),
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.content, "raw CRUD memory one updated");
    assert_eq!(updated.tags, vec!["legal-tag"]);
    assert_eq!(updated.entities, vec!["legal-entity"]);
    let canonical_path = VaultPath::parse(updated.canonical_path.as_deref().unwrap()).unwrap();
    let updated_file = core
        .read_managed(&context, &canonical_path)
        .await
        .unwrap()
        .file;
    assert_eq!(
        updated_file.current_revision.value(),
        updated.canonical_revision.unwrap()
    );
    let revisions = state
        .files()
        .list_revisions(&context, updated_file.id)
        .await
        .unwrap();
    let update_revision = revisions.last().unwrap();
    assert_eq!(update_revision.source_plane, SourcePlane::Mcp);
    assert_eq!(
        update_revision.actor_id,
        Some(ActorId::new("crud-agent").unwrap())
    );

    let too_many_tags = facade
        .update(
            &context,
            &core,
            actor(),
            SourcePlane::Mcp,
            &SemanticExplicitUpdateRequest {
                memory_id: ids[0].clone(),
                expected_revision: updated.revision,
                patch: SemanticExplicitUpdatePatch {
                    tags: Some((0..65).map(|index| format!("tag-{index}")).collect()),
                    ..Default::default()
                },
            },
        )
        .await;
    assert!(matches!(
        too_many_tags,
        Err(mcp_vault_memory::MemoryError::InvalidInput(
            "memory metadata is too large"
        ))
    ));
    let invalid_entity = facade
        .update(
            &context,
            &core,
            actor(),
            SourcePlane::Mcp,
            &SemanticExplicitUpdateRequest {
                memory_id: ids[0].clone(),
                expected_revision: updated.revision,
                patch: SemanticExplicitUpdatePatch {
                    entities: Some(vec!["invalid\u{0001}".to_owned()]),
                    ..Default::default()
                },
            },
        )
        .await;
    assert!(matches!(
        invalid_entity,
        Err(mcp_vault_memory::MemoryError::InvalidInput(
            "memory tag/entity is invalid"
        ))
    ));
    let after_invalid_updates = facade.get(&context, &ids[0]).await.unwrap().unwrap();
    assert_eq!(after_invalid_updates.revision, updated.revision);
    assert_eq!(after_invalid_updates.tags, vec!["legal-tag"]);
    assert_eq!(after_invalid_updates.entities, vec!["legal-entity"]);

    let stale = facade
        .update(
            &context,
            &core,
            actor(),
            SourcePlane::Mcp,
            &SemanticExplicitUpdateRequest {
                memory_id: ids[0].clone(),
                expected_revision: 1,
                patch: SemanticExplicitUpdatePatch {
                    content: Some("must not win".to_owned()),
                    ..Default::default()
                },
            },
        )
        .await;
    assert!(matches!(
        stale,
        Err(mcp_vault_memory::MemoryError::Conflict)
    ));
    assert_eq!(
        facade
            .get(&context, &ids[0])
            .await
            .unwrap()
            .unwrap()
            .content,
        "raw CRUD memory one updated"
    );

    let second = facade.get(&context, &ids[1]).await.unwrap().unwrap();
    let second_path = VaultPath::parse(second.canonical_path.as_deref().unwrap()).unwrap();
    let second_file = core
        .read_managed(&context, &second_path)
        .await
        .unwrap()
        .file;
    let deleted = facade
        .delete(
            &context,
            &core,
            actor(),
            SourcePlane::Mcp,
            &SemanticExplicitDeleteRequest {
                memory_id: ids[1].clone(),
                expected_revision: second.revision,
                idempotency_key: "raw-crud-delete-once".to_owned(),
            },
        )
        .await
        .unwrap();
    assert!(deleted.deleted);
    assert_eq!(deleted.ownership, "explicit");
    assert!(facade.get(&context, &ids[1]).await.unwrap().is_none());
    let replay = facade
        .delete(
            &context,
            &core,
            actor(),
            SourcePlane::Mcp,
            &SemanticExplicitDeleteRequest {
                memory_id: ids[1].clone(),
                expected_revision: second.revision,
                idempotency_key: "raw-crud-delete-once".to_owned(),
            },
        )
        .await
        .unwrap();
    assert!(replay.deleted);
    let conflict = facade
        .delete(
            &context,
            &core,
            actor(),
            SourcePlane::Mcp,
            &SemanticExplicitDeleteRequest {
                memory_id: ids[1].clone(),
                expected_revision: second.revision + 1,
                idempotency_key: "raw-crud-delete-once".to_owned(),
            },
        )
        .await;
    assert!(matches!(
        conflict,
        Err(mcp_vault_memory::MemoryError::Conflict)
    ));
    let canonical_revisions = state
        .files()
        .list_revisions(&context, second_file.id)
        .await
        .unwrap();
    let delete_revision = canonical_revisions.last().unwrap();
    assert_eq!(delete_revision.source_plane, SourcePlane::Mcp);
    assert_eq!(
        delete_revision.actor_id,
        Some(ActorId::new("crud-agent").unwrap())
    );

    let foreign_context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("foreign-crud").unwrap(),
        _dir.path().join("foreign-crud-vault"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&foreign_context, "foreign-crud", VaultStatus::Active)
        .await
        .unwrap();
    assert!(
        facade
            .get(&foreign_context, &ids[0])
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        facade
            .list(&foreign_context, &SemanticExplicitListRequest::default())
            .await
            .unwrap()
            .memories
            .is_empty()
    );
    let foreign_update = facade
        .update(
            &foreign_context,
            &core,
            actor(),
            SourcePlane::Mcp,
            &SemanticExplicitUpdateRequest {
                memory_id: ids[0].clone(),
                expected_revision: 2,
                patch: SemanticExplicitUpdatePatch {
                    content: Some("must not cross the Vault boundary".to_owned()),
                    ..Default::default()
                },
            },
        )
        .await;
    assert!(matches!(
        foreign_update,
        Err(mcp_vault_memory::MemoryError::NotFound)
    ));
    let foreign_delete = facade
        .delete(
            &foreign_context,
            &core,
            actor(),
            SourcePlane::Mcp,
            &SemanticExplicitDeleteRequest {
                memory_id: ids[0].clone(),
                expected_revision: 2,
                idempotency_key: "foreign-crud-delete".to_owned(),
            },
        )
        .await;
    assert!(matches!(
        foreign_delete,
        Err(mcp_vault_memory::MemoryError::NotFound)
    ));
    let owner_after_foreign_attempts = facade.get(&context, &ids[0]).await.unwrap().unwrap();
    assert_eq!(owner_after_foreign_attempts.revision, 2);
    assert_eq!(
        owner_after_foreign_attempts.content,
        "raw CRUD memory one updated"
    );
    assert!(
        SemanticMemoryService::new(state.clone())
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .is_empty()
    );
    let pack = mcp_vault_memory::MemoryPackService::new(state)
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "raw CRUD memory".to_owned(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(pack.current_context.is_empty() && pack.relevant_experiences.is_empty());
}

#[test]
fn explicit_request_schema_matches_existing_runtime_limits() {
    let schema =
        serde_json::to_value(schemars::schema_for!(SemanticRememberExplicitRequest)).unwrap();
    let properties = &schema["properties"];
    for name in ["importance", "confidence"] {
        assert_eq!(properties[name]["minimum"].as_f64(), Some(0.0), "{name}");
        assert_eq!(properties[name]["maximum"].as_f64(), Some(1.0), "{name}");
    }
    for name in ["tags", "entities"] {
        assert_eq!(properties[name]["maxItems"].as_u64(), Some(64), "{name}");
    }
    assert_eq!(properties["sources"]["maxItems"].as_u64(), Some(32));

    let update_schema =
        serde_json::to_value(schemars::schema_for!(SemanticExplicitUpdatePatch)).unwrap();
    let update_properties = &update_schema["properties"];
    for name in ["importance", "confidence"] {
        assert_eq!(
            update_properties[name]["minimum"].as_f64(),
            Some(0.0),
            "update {name}"
        );
        assert_eq!(
            update_properties[name]["maximum"].as_f64(),
            Some(1.0),
            "update {name}"
        );
    }
    for name in ["tags", "entities"] {
        assert_eq!(
            update_properties[name]["maxItems"].as_u64(),
            Some(64),
            "update {name}"
        );
    }
}
