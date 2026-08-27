use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{Json, Router, extract::State as AxumState, http::StatusCode, routing::post};
use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_core::VaultCore;
use mcp_vault_domain::{
    Actor, MemoryConsolidationId, MemoryId, MemoryRawId, ModelId, ProviderId, Revision,
    SourcePlane, VaultContext, VaultId, VaultPath, VaultSlug, WritePrecondition,
};
use mcp_vault_memory::{
    ExtractionPolicy, ExtractionSourceMode, MEMORY_PIPELINE_GENERATION, MemoryOrigin,
    MemoryService, MemorySourceInput, MemoryStatus, MemoryType, MemoryUpdateInput, MemoryView,
    NoteExtractionOptions, RecallContext, RecallRequest, RememberInput,
};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderMode,
    ProviderService, ProviderSettings,
};
use mcp_vault_state::{
    MemoryBundle, MemoryConsolidationProposalRecord, MemoryFilter, MemoryRecord,
    MemoryStage1OutputRecord, StateStore, VaultStatus,
};
use mcp_vault_storage_fs::StorageOptions;
use serde_json::json;
use tempfile::TempDir;
use tokio::io::AsyncReadExt;

async fn fake_extraction(
    AxumState(calls): AxumState<Arc<AtomicUsize>>,
    Json(request): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    if request["model"] == "fake-consolidator" {
        return fake_consolidation_response(&request);
    }
    let call = calls.fetch_add(1, Ordering::SeqCst);
    let schema = &request["response_format"]["json_schema"]["schema"];
    if request["max_tokens"] != 8_192
        || request["response_format"]["type"] != "json_schema"
        || schema["properties"]
            .as_object()
            .is_none_or(|properties| properties.len() != 3)
        || schema["required"]
            .as_array()
            .is_none_or(|required| required.len() != 3)
        || schema["properties"].get("evidence").is_some()
        || !request["messages"][0]["content"]
            .as_str()
            .is_some_and(|prompt| prompt.contains("Phase 1 memory writing model"))
        || !request["messages"][1]["content"]
            .as_str()
            .is_some_and(|content| {
                content.contains("A durable decision is documented here.")
                    && !content.contains("L1:")
            })
    {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "unexpected extraction request contract"})),
        );
    }
    let content = if call == 1 {
        "{\"rollout_summary\":\"The note records a durable decision.\",\"rollout_slug\":\"durable-decision\",\"raw_memory\":\"The project has a documented durable decision.\",\"evidence\":[]}"
    } else {
        "{\"rollout_summary\":\"The note records a durable decision.\",\"rollout_slug\":\"durable-decision\",\"raw_memory\":\"The project has a documented durable decision.\"}"
    };
    (
        StatusCode::OK,
        Json(json!({
            "choices": [{
                "message": {
                    "content": content
                }
            }]
        })),
    )
}

async fn fake_consolidation(
    Json(request): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    fake_consolidation_response(&request)
}

fn fake_consolidation_response(
    request: &serde_json::Value,
) -> (StatusCode, Json<serde_json::Value>) {
    let Some(user) = request["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_str())
    else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "missing consolidation input"})),
        );
    };
    let Some(payload) = user
        .strip_prefix("<untrusted_memory_state>\n")
        .and_then(|value| value.strip_suffix("\n</untrusted_memory_state>"))
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
    else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "invalid consolidation input"})),
        );
    };
    let dirty = payload["dirty_inputs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let raw_memories = payload["raw_memories"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let current = payload["current_memories"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut actions = Vec::new();
    let discarded_input_indexes = Vec::<serde_json::Value>::new();
    for dirty_input in dirty {
        let input_index = dirty_input["input_index"].as_u64().unwrap();
        match dirty_input["status"].as_str().unwrap_or_default() {
            "no_output" => {}
            "withdrawn" => {
                for memory in &current {
                    if memory["support_input_indexes"]
                        .as_array()
                        .is_some_and(|indexes| {
                            indexes
                                .iter()
                                .any(|index| index.as_u64() == Some(input_index))
                        })
                    {
                        actions.push(json!({
                            "operation": "archive",
                            "memory_index": memory["memory_index"],
                            "content": null,
                            "memory_type": null,
                            "input_indexes": [],
                            "supersedes_memory_indexes": []
                        }));
                    }
                }
            }
            "ready" => {
                let raw = raw_memories
                    .iter()
                    .find(|raw| raw["input_index"].as_u64() == Some(input_index))
                    .unwrap();
                let supersedes = raw["metadata"]["requested_supersedes_memory_index"]
                    .as_u64()
                    .map_or_else(Vec::new, |index| vec![json!(index)]);
                let existing = (supersedes.is_empty())
                    .then(|| {
                        current.iter().find(|memory| {
                            memory["support_input_indexes"]
                                .as_array()
                                .is_some_and(|indexes| {
                                    indexes
                                        .iter()
                                        .any(|index| index.as_u64() == Some(input_index))
                                })
                                || memory["content"] == raw["raw_memory"]
                        })
                    })
                    .flatten();
                actions.push(json!({
                    "operation": if existing.is_some() { "update" } else { "create" },
                    "memory_index": existing.map_or(serde_json::Value::Null, |memory| {
                        memory["memory_index"].clone()
                    }),
                    "content": raw["raw_memory"],
                    "memory_type": raw["metadata"]["memory_type"].as_str().unwrap_or("decision"),
                    "input_indexes": [input_index],
                    "supersedes_memory_indexes": supersedes
                }));
            }
            _ => unreachable!(),
        }
    }
    let summary = actions
        .iter()
        .filter_map(|action| action["content"].as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let content = json!({
        "memory_summary": summary,
        "actions": actions,
        "discarded_input_indexes": discarded_input_indexes,
    })
    .to_string();
    (
        StatusCode::OK,
        Json(json!({"choices": [{"message": {"content": content}}]})),
    )
}

struct FinalizedRememberResult {
    outcome: String,
    memory: MemoryView,
}

async fn configure_fake_consolidation(state: &StateStore, context: &VaultContext) {
    if state
        .providers()
        .resolve_binding(context, "memory_consolidation")
        .await
        .unwrap()
        .is_some()
    {
        return;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/v1/chat/completions", post(fake_consolidation)),
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
        .set_provider_mode(context, ProviderMode::LocalOnly, None)
        .await
        .unwrap();
    let provider = providers
        .create_provider(ProviderInput {
            name: format!("fake-consolidation-{}", context.id()),
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
            external_model_id: "fake-consolidator".to_owned(),
            capabilities: ModelCapabilities {
                structured_output: true,
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
            "memory_consolidation",
            model.id,
            json!({}),
            None,
        )
        .await
        .unwrap();
}

async fn remember_and_consolidate(
    service: &MemoryService,
    context: &VaultContext,
    core: &VaultCore,
    input: RememberInput,
) -> FinalizedRememberResult {
    configure_fake_consolidation(service.state(), context).await;
    let staged = service.remember(context, core, input).await.unwrap();
    assert!(staged.memory.is_none());
    let raw_id = staged.raw_memory_id.unwrap();
    service.consolidate(context, core).await.unwrap();
    let records = service
        .state()
        .memory()
        .list_memories(context, &MemoryFilter::default(), 200, 0)
        .await
        .unwrap();
    let raw_id = raw_id.to_string();
    let record = records
        .into_iter()
        .find(|record| {
            record.extraction["stage1_ids"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(raw_id.as_str())))
        })
        .expect("consolidation should materialize the staged input");
    FinalizedRememberResult {
        outcome: staged.outcome,
        memory: service.get(context, record.id).await.unwrap(),
    }
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
    state
        .memory()
        .set_pipeline_generation_state(&context, MEMORY_PIPELINE_GENERATION, false)
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
async fn extraction_policy_is_typed_and_vault_scoped() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (first, _first_core, service) = fixture(&state, &directory, "first-policy").await;
    let (second, _second_core, _) = fixture(&state, &directory, "second-policy").await;

    let initial = service.extraction_policy(&first).await.unwrap();
    assert!(!initial.policy.enabled);
    assert_eq!(initial.policy.request_timeout_seconds, 300);
    assert!(initial.revision.is_none());
    let updated = service
        .set_extraction_policy(
            &first,
            ExtractionPolicy {
                enabled: true,
                ..ExtractionPolicy::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(updated.revision, Some(Revision::new(1)));
    assert!(
        service
            .extraction_policy(&first)
            .await
            .unwrap()
            .policy
            .enabled
    );
    assert!(
        !service
            .extraction_policy(&second)
            .await
            .unwrap()
            .policy
            .enabled
    );
    let legacy_limit = service
        .set_extraction_policy(
            &first,
            ExtractionPolicy {
                enabled: true,
                max_evidence_per_note: 11,
                ..ExtractionPolicy::default()
            },
            Some(Revision::new(1)),
            None,
        )
        .await
        .unwrap();
    assert_eq!(legacy_limit.revision, Some(Revision::new(2)));
    assert!(
        service
            .set_extraction_policy(
                &first,
                ExtractionPolicy {
                    request_timeout_seconds: 29,
                    ..ExtractionPolicy::default()
                },
                Some(Revision::new(2)),
                None,
            )
            .await
            .is_err()
    );
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
    let created = remember_and_consolidate(&service, &context, &core, input.clone()).await;
    assert_eq!(created.outcome, "staged");
    assert_eq!(created.memory.status, MemoryStatus::Active);
    assert!(created.memory.canonical_path.is_some());

    let path = created.memory.canonical_path.clone().unwrap();
    let managed = core.read_managed(&context, &path).await.unwrap();
    let mut reader = managed.reader;
    let mut body = String::new();
    reader.read_to_string(&mut body).await.unwrap();
    assert!(body.contains("The service preserves WebDAV revisions."));
    assert!(body.contains("status: \"active\""));

    let repeated = remember_and_consolidate(&service, &context, &core, input).await;
    assert_eq!(repeated.outcome, "staged_existing");
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
    assert!(recalled.memories[0].sources.is_empty());
    assert_eq!(
        service
            .get(&context, created.memory.id)
            .await
            .unwrap()
            .sources
            .len(),
        1
    );

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
    let created = remember_and_consolidate(
        &service,
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
    .await;
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
    let extracted = remember_and_consolidate(
        &service,
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
                start_line: Some(1),
                end_line: Some(1),
                ..MemorySourceInput::default()
            }],
            supersedes: None,
            idempotency_key: None,
            origin: MemoryOrigin::Extracted,
            extraction: json!({"pipeline_version": 1}),
        },
    )
    .await;
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
async fn changed_source_reconsolidates_same_memory_without_an_intermediate_stale_state() {
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
            start_line: Some(1),
            end_line: Some(1),
            ..MemorySourceInput::default()
        }],
        origin: MemoryOrigin::Extracted,
        ..RememberInput::default()
    };
    let created = remember_and_consolidate(
        &service,
        &context,
        &core,
        input(source.file.current_revision),
    )
    .await;
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
            .get(&context, created.memory.id)
            .await
            .unwrap()
            .status,
        MemoryStatus::Active
    );

    let revived = remember_and_consolidate(
        &service,
        &context,
        &core,
        input(changed.file.current_revision),
    )
    .await;
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
    let memory = remember_and_consolidate(
        &service,
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
                    start_line: Some(1),
                    end_line: Some(1),
                    ..MemorySourceInput::default()
                },
                MemorySourceInput {
                    source_type: "note".to_owned(),
                    note_file_id: Some(second.file.id),
                    note_path: Some(second_path),
                    note_revision: Some(second.file.current_revision),
                    start_line: Some(1),
                    end_line: Some(1),
                    ..MemorySourceInput::default()
                },
            ],
            origin: MemoryOrigin::Extracted,
            ..RememberInput::default()
        },
    )
    .await;
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
    let old = remember_and_consolidate(
        &service,
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
    .await;
    let newer = remember_and_consolidate(
        &service,
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
    .await;
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

    let old_revision = service.get(&context, old.memory.id).await.unwrap().revision;
    let newer_revision = service
        .get(&context, newer.memory.id)
        .await
        .unwrap()
        .revision;
    let first_rebuild = service.rebuild(&context, &core).await.unwrap();
    let second_rebuild = service.rebuild(&context, &core).await.unwrap();
    assert_eq!(first_rebuild.quarantined, 0);
    assert_eq!(second_rebuild.quarantined, 0);
    assert_eq!(
        service.get(&context, old.memory.id).await.unwrap().revision,
        old_revision
    );
    let after_rebuild = service.get(&context, newer.memory.id).await.unwrap();
    assert_eq!(after_rebuild.revision, newer_revision);
    assert_eq!(after_rebuild.relations.len(), 1);
}

#[tokio::test]
async fn invalid_managed_markdown_is_quarantined_without_becoming_recallable() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (context, core, service) = fixture(&state, &directory, "quarantine").await;
    let created = remember_and_consolidate(
        &service,
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
    .await;
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
    let created = remember_and_consolidate(
        &service,
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
    .await;
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
async fn ordinary_notes_reach_automatic_provider_boundary_and_legacy_modes_migrate() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (context, core, service) = fixture(&state, &directory, "source-intent").await;
    let note = core
        .create_bytes(
            &context,
            &VaultPath::parse("notes/reference.md").unwrap(),
            b"# Reference\n\nA generic implementation detail.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    service
        .set_extraction_policy(
            &context,
            ExtractionPolicy {
                enabled: true,
                ..ExtractionPolicy::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        service.extract_note(&context, &core, &note.file.path).await,
        Err(mcp_vault_memory::MemoryError::Configuration(
            "memory_extraction_model_unbound"
        ))
    ));

    let legacy: ExtractionPolicy = serde_json::from_value(json!({
        "enabled": true,
        "source_mode": "all_notes"
    }))
    .unwrap();
    assert_eq!(legacy.source_mode, ExtractionSourceMode::Automatic);
    let explicit_legacy: ExtractionPolicy = serde_json::from_value(json!({
        "enabled": true,
        "source_mode": "explicit_only"
    }))
    .unwrap();
    assert_eq!(explicit_legacy.source_mode, ExtractionSourceMode::Automatic);
}

#[tokio::test]
async fn ordinary_note_extraction_stages_then_consolidates_semantic_memory() {
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
    let calls = Arc::new(AtomicUsize::new(0));
    let server = tokio::spawn({
        let calls = calls.clone();
        async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/v1/chat/completions", post(fake_extraction))
                    .with_state(calls),
            )
            .await
            .unwrap();
        }
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
    let consolidation_model = providers
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "fake-consolidator".to_owned(),
            capabilities: ModelCapabilities {
                structured_output: true,
                ..ModelCapabilities::default()
            },
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await
        .unwrap();
    providers
        .bind_model(
            Some(&context),
            "memory_consolidation",
            consolidation_model.id,
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
            &json!({"enabled": true}),
            WritePrecondition::Unconditional,
            None,
        )
        .await
        .unwrap();

    let extracted = service
        .extract_note(&context, &core, &note.file.path)
        .await
        .unwrap();
    assert!(extracted.source_admitted);
    assert!(extracted.raw_memory_staged);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
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
                0,
            )
            .await
            .unwrap()
            .is_empty()
    );
    let staged = state
        .memory()
        .get_stage1_output(&context, "note", &note.file.id.to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        staged.raw_memory,
        "The project has a documented durable decision."
    );
    assert!(!staged.evidence.to_string().contains("A durable decision"));
    let (other_context, _other_core, _other_service) =
        fixture(&state, &directory, "extract-other").await;
    assert!(
        state
            .memory()
            .get_stage1_output(&other_context, "note", &note.file.id.to_string())
            .await
            .unwrap()
            .is_none()
    );
    let consolidated = service.consolidate(&context, &core).await.unwrap();
    assert_eq!(consolidated.created, 1);
    assert_eq!(consolidated.generation, 1);
    let memories = service
        .list(
            &context,
            vec![MemoryStatus::Active],
            Vec::new(),
            None,
            None,
            None,
            10,
            0,
        )
        .await
        .unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(
        memories[0].content,
        "The project has a documented durable decision."
    );
    assert_eq!(memories[0].sources[0].start_line, None);
    assert!(
        state
            .memory()
            .get_bundle(&context, memories[0].id)
            .await
            .unwrap()
            .unwrap()
            .sources[0]
            .excerpt_hash
            .is_some()
    );
    let canonical_path = memories[0].canonical_path.clone().unwrap();
    let mut canonical = core.read_managed(&context, &canonical_path).await.unwrap();
    let mut markdown = String::new();
    canonical
        .reader
        .read_to_string(&mut markdown)
        .await
        .unwrap();
    assert!(markdown.contains("The project has a documented durable decision."));
    let mut global = core
        .read_managed(
            &context,
            &VaultPath::parse("_mcp-vault/memory/MEMORY.md").unwrap(),
        )
        .await
        .unwrap();
    let mut global_markdown = String::new();
    global
        .reader
        .read_to_string(&mut global_markdown)
        .await
        .unwrap();
    assert!(global_markdown.contains("The project has a documented durable decision."));
    assert!(!global_markdown.contains("A durable decision is documented here."));

    let repeated = service
        .extract_note(&context, &core, &note.file.path)
        .await
        .unwrap();
    assert!(!repeated.source_admitted);
    assert!(repeated.already_evaluated);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let forced_error = service
        .extract_note_with_options(
            &context,
            &core,
            &note.file.path,
            NoteExtractionOptions {
                include_evaluated: true,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(forced_error.code(), "provider_schema_invalid");
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    let retried_after_force_failure = service
        .extract_note(&context, &core, &note.file.path)
        .await
        .unwrap();
    assert!(retried_after_force_failure.source_admitted);
    assert!(!retried_after_force_failure.already_evaluated);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        state.memory().pending_stage1_count(&context).await.unwrap(),
        0
    );

    service
        .set_extraction_policy(
            &context,
            ExtractionPolicy {
                enabled: true,
                max_evidence_per_note: 2,
                ..ExtractionPolicy::default()
            },
            Some(Revision::new(1)),
            None,
        )
        .await
        .unwrap();
    let changed_profile = service
        .extract_note(&context, &core, &note.file.path)
        .await
        .unwrap();
    assert!(changed_profile.already_evaluated);
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    let changed_note = core
        .replace_bytes(
            &context,
            &note.file.path,
            note.file.current_revision,
            b"A durable decision is documented here.\n\nAdditional context.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let changed_revision = service
        .extract_note(&context, &core, &changed_note.file.path)
        .await
        .unwrap();
    assert!(changed_revision.source_admitted);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!(
        state.memory().pending_stage1_count(&context).await.unwrap(),
        1
    );
    let consolidated_update = service.consolidate(&context, &core).await.unwrap();
    assert_eq!(consolidated_update.updated, 1);
    let after_update = service.get(&context, memories[0].id).await.unwrap();
    assert_eq!(after_update.id, memories[0].id);
    assert_eq!(
        after_update.sources[0].revision,
        Some(changed_note.file.current_revision)
    );
    server.abort();
}

#[tokio::test]
async fn consolidation_reuses_prepared_proposal_after_partial_artifact_failure() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (context, core, service) = fixture(&state, &directory, "prepared-recovery").await;
    configure_fake_consolidation(&state, &context).await;
    let staged = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: "Admin authentication remains mandatory.".to_owned(),
                memory_type: MemoryType::Decision,
                origin: MemoryOrigin::ExplicitAdmin,
                ..RememberInput::default()
            },
        )
        .await
        .unwrap();
    assert!(staged.raw_memory_id.is_some());

    let blocked_global = context.content_root().join("_mcp-vault/memory/MEMORY.md");
    tokio::fs::create_dir_all(&blocked_global).await.unwrap();
    let first = service.consolidate(&context, &core).await.unwrap_err();
    assert_eq!(first.code(), "memory_core_error");
    assert_eq!(
        state.memory().pending_stage1_count(&context).await.unwrap(),
        1
    );
    assert!(
        state
            .memory()
            .latest_prepared_consolidation_proposal(&context)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(state.memory().counts(&context).await.unwrap().active, 1);
    let blocked_mutation = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: "This must wait for prepared recovery.".to_owned(),
                memory_type: MemoryType::Decision,
                ..RememberInput::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(blocked_mutation.code(), "memory_conflict");

    tokio::fs::remove_dir(&blocked_global).await.unwrap();
    let recovered = service.consolidate(&context, &core).await.unwrap();
    assert!(recovered.reused_proposal);
    assert_eq!(recovered.generation, 1);
    assert_eq!(
        state.memory().pending_stage1_count(&context).await.unwrap(),
        0
    );
    assert_eq!(state.memory().counts(&context).await.unwrap().active, 1);
    assert!(
        state
            .memory()
            .latest_prepared_consolidation_proposal(&context)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn obsolete_prepared_contract_is_rejected_before_parsing_or_provider_use() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (context, core, service) = fixture(&state, &directory, "obsolete-proposal").await;
    let input_hash = "sha256:obsolete-contract";
    state
        .memory()
        .insert_consolidation_proposal(
            &context,
            &MemoryConsolidationProposalRecord {
                id: MemoryConsolidationId::new(),
                vault_id: context.id(),
                input_hash: input_hash.to_owned(),
                proposal: json!({"intentionally": "not a current typed proposal"}),
                model_id: ModelId::new(),
                provider_id: ProviderId::new(),
                prompt_version: "memory-consolidation-v3".to_owned(),
                status: "prepared".to_owned(),
                created_at: 1,
                applied_at: None,
            },
        )
        .await
        .unwrap();

    service.refresh_artifacts(&context, &core).await.unwrap();
    let report = service.consolidate(&context, &core).await.unwrap();
    assert_eq!(report.raw_inputs, 0);
    assert_eq!(
        state
            .memory()
            .get_consolidation_proposal_by_input(&context, input_hash)
            .await
            .unwrap()
            .unwrap()
            .status,
        "rejected"
    );
}

#[tokio::test]
async fn pipeline_reset_discards_all_old_memory_state_and_managed_files() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (context, core, service) = fixture(&state, &directory, "legacy-reset").await;
    let source = core
        .create_bytes(
            &context,
            &VaultPath::parse("notes/legacy.md").unwrap(),
            b"The old pipeline copied this source quote.",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let extracted = remember_and_consolidate(
        &service,
        &context,
        &core,
        RememberInput {
            content: "The old pipeline copied this source quote.".to_owned(),
            memory_type: MemoryType::Fact,
            sources: vec![MemorySourceInput {
                source_type: "note".to_owned(),
                note_file_id: Some(source.file.id),
                note_path: Some(source.file.path.clone()),
                note_revision: Some(source.file.current_revision),
                start_line: Some(1),
                end_line: Some(1),
                ..MemorySourceInput::default()
            }],
            origin: MemoryOrigin::Extracted,
            ..RememberInput::default()
        },
    )
    .await;
    let explicit = remember_and_consolidate(
        &service,
        &context,
        &core,
        RememberInput {
            content: "Admin authentication remains enabled.".to_owned(),
            memory_type: MemoryType::Decision,
            origin: MemoryOrigin::ExplicitAdmin,
            ..RememberInput::default()
        },
    )
    .await;
    let extracted_path = extracted.memory.canonical_path.clone().unwrap();
    let old_note_raw = state
        .memory()
        .list_stage1_outputs(&context, false, 4096)
        .await
        .unwrap()
        .into_iter()
        .find(|raw| raw.source_type == "note")
        .unwrap();
    let old_source_summary = VaultPath::parse(&format!(
        "_mcp-vault/memory/source_summaries/{}.md",
        old_note_raw.id
    ))
    .unwrap();
    let (other_context, other_core, other_service) =
        fixture(&state, &directory, "pipeline-reset-other").await;
    let other = remember_and_consolidate(
        &other_service,
        &other_context,
        &other_core,
        RememberInput {
            content: "The other Vault must survive this cutover.".to_owned(),
            memory_type: MemoryType::Decision,
            origin: MemoryOrigin::ExplicitAdmin,
            ..RememberInput::default()
        },
    )
    .await;
    let other_path = other.memory.canonical_path.clone().unwrap();

    state
        .memory()
        .invalidate_pipeline_generation(&context)
        .await
        .unwrap();
    let partial = core.read_managed(&context, &extracted_path).await.unwrap();
    core.delete_managed(
        &context,
        &extracted_path,
        partial.file.current_revision,
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let report = service.reset_pipeline(&context, &core).await.unwrap();
    assert!(report.removed_managed_files >= 4);
    assert_eq!(report.cleared_memories, 2);
    assert_eq!(report.cleared_stage1_outputs, 2);
    assert!(
        state
            .memory()
            .get_memory(&context, extracted.memory.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(core.read_managed(&context, &extracted_path).await.is_err());
    assert!(
        state
            .memory()
            .get_memory(&context, explicit.memory.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        core.read_managed(&context, &old_source_summary)
            .await
            .is_err()
    );
    assert_eq!(
        state.memory().stage1_counts(&context).await.unwrap().total,
        0
    );
    assert!(
        state
            .memory()
            .get_memory(&other_context, other.memory.id)
            .await
            .unwrap()
            .is_some()
    );
    other_core
        .read_managed(&other_context, &other_path)
        .await
        .unwrap();
    let reset_state = state
        .memory()
        .get_consolidation_state(&context)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reset_state.pipeline_generation, MEMORY_PIPELINE_GENERATION);
    assert!(reset_state.regeneration_pending);
    let mut global = core
        .read_managed(
            &context,
            &VaultPath::parse("_mcp-vault/memory/MEMORY.md").unwrap(),
        )
        .await
        .unwrap();
    let mut global_markdown = String::new();
    global
        .reader
        .read_to_string(&mut global_markdown)
        .await
        .unwrap();
    assert!(!global_markdown.contains("old pipeline copied"));
    assert!(!global_markdown.contains("Admin authentication remains enabled."));

    let repeated = service.reset_pipeline(&context, &core).await.unwrap();
    assert!(repeated.already_completed);
    assert_eq!(repeated.removed_managed_files, 0);
}

#[tokio::test]
async fn global_and_raw_memory_artifacts_include_rows_beyond_one_state_page() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let (context, core, service) = fixture(&state, &directory, "global-artifact-pages").await;

    for index in 0..=200_u32 {
        let content = format!("Durable paginated memory {index}.");
        state
            .memory()
            .replace_bundle(
                &context,
                &MemoryBundle {
                    memory: MemoryRecord {
                        id: MemoryId::new(),
                        vault_id: context.id(),
                        memory_type: MemoryType::Fact.as_str().to_owned(),
                        status: MemoryStatus::Active.as_str().to_owned(),
                        content: content.clone(),
                        normalized_content: content.to_lowercase(),
                        content_hash: format!("test-hash-{index}"),
                        importance: 0.8,
                        confidence: 1.0,
                        origin: MemoryOrigin::ExplicitAgent.as_str().to_owned(),
                        revision: Revision::new(1),
                        canonical_file_id: None,
                        canonical_path: None,
                        canonical_revision: None,
                        valid_from: None,
                        valid_to: None,
                        extraction: json!({"pipeline": "codex_two_phase"}),
                        created_at: i64::from(index),
                        updated_at: i64::from(index),
                        last_recalled_at: None,
                        recall_count: 0,
                    },
                    sources: Vec::new(),
                    entities: Vec::new(),
                    tags: Vec::new(),
                    relations: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        state
            .memory()
            .upsert_stage1_output(
                &context,
                &MemoryStage1OutputRecord {
                    id: MemoryRawId::new(),
                    vault_id: context.id(),
                    source_type: "explicit_agent".to_owned(),
                    source_key: format!("paginated-source-{index}"),
                    source_file_id: None,
                    source_path: None,
                    source_revision: None,
                    profile_hash: "pagination-test".to_owned(),
                    pipeline_version: 8,
                    prompt_version: "pagination-test".to_owned(),
                    raw_memory: format!("Durable paginated raw memory {index}."),
                    source_summary: format!("Summary for paginated raw memory {index}."),
                    source_slug: Some(format!("paginated-raw-{index}")),
                    evidence: json!([]),
                    metadata: json!({}),
                    output_hash: format!("raw-test-hash-{index}"),
                    status: "ready".to_owned(),
                    generated_at: i64::from(index),
                    updated_at: i64::from(index),
                    usage_count: 0,
                    last_usage: None,
                    selected_for_phase2: false,
                    selected_for_phase2_hash: None,
                    selected_for_phase2_at: None,
                },
            )
            .await
            .unwrap();
    }

    service.refresh_artifacts(&context, &core).await.unwrap();
    let mut global = core
        .read_managed(
            &context,
            &VaultPath::parse("_mcp-vault/memory/MEMORY.md").unwrap(),
        )
        .await
        .unwrap();
    let mut global_markdown = String::new();
    global
        .reader
        .read_to_string(&mut global_markdown)
        .await
        .unwrap();
    assert!(global_markdown.contains("Durable paginated memory 0."));
    assert!(global_markdown.contains("Durable paginated memory 200."));
    assert_eq!(
        global_markdown.matches("Durable paginated memory ").count(),
        201
    );

    let mut raw = core
        .read_managed(
            &context,
            &VaultPath::parse("_mcp-vault/memory/raw_memories.md").unwrap(),
        )
        .await
        .unwrap();
    let mut raw_markdown = String::new();
    raw.reader.read_to_string(&mut raw_markdown).await.unwrap();
    assert!(raw_markdown.contains("Durable paginated raw memory 0."));
    assert!(raw_markdown.contains("Durable paginated raw memory 200."));
    assert_eq!(
        raw_markdown
            .matches("Durable paginated raw memory ")
            .count(),
        201
    );
}
