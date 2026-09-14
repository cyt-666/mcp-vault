use axum::{Json, Router, extract::State as AxumState, routing::post};
use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_core::VaultCore;
use mcp_vault_domain::{Actor, Revision, SourcePlane, VaultContext, VaultId, VaultPath, VaultSlug};
use mcp_vault_memory::v3::{
    ExtractionPolicy, MemoryService, MemoryType, MemoryUpdateInput, NoteExtractionOptions,
    RememberInput,
};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderMode,
    ProviderService, ProviderSettings,
};
use mcp_vault_state::{StateStore, UnitFilter, VaultStatus};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::io::AsyncReadExt;

async fn fixture() -> (
    tempfile::TempDir,
    StateStore,
    VaultContext,
    VaultCore,
    MemoryService,
) {
    let dir = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("test").unwrap(),
        dir.path().join("vault"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&context, "test", VaultStatus::Active)
        .await
        .unwrap();
    let core = VaultCore::new(
        state.clone(),
        dir.path().join("history"),
        Default::default(),
        Default::default(),
        Default::default(),
    );
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[23u8; 32]).unwrap(),
    );
    let service = MemoryService::new(state.clone(), auth);
    (dir, state, context, core, service)
}

async fn select_all(
    AxumState(calls): AxumState<Arc<AtomicUsize>>,
    Json(request): Json<Value>,
) -> Json<Value> {
    let call_number = calls.fetch_add(1, Ordering::SeqCst) + 1;
    if request["response_format"]["json_schema"]["name"] == "memory_overview" {
        let input: Value =
            serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
        return Json(
            json!({"choices":[{"message":{"content":json!({"sections":[{"label":"Operational context","description":"Read the source units for adopted procedures and conditions.","unit_ids":input["units"].as_array().unwrap().iter().map(|unit|unit["id"].clone()).collect::<Vec<_>>()}]}).to_string()}}]}),
        );
    }
    if request["response_format"]["json_schema"]["name"]
        == "memory-source-unit-completeness-review-v1"
    {
        let input: Value =
            serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
        let mode = input["source_path"].as_str().unwrap_or_default();
        if mode.starts_with("review-") {
            let units = input["units"].as_array().unwrap();
            assert!(units.iter().any(|unit| {
                unit["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("迁移前必须验证备份可恢复"))
            }));
            let first = input["first_selected"].as_array().unwrap();
            let selected = first.first().cloned().unwrap_or(Value::Null);
            if mode == "review-invalid.md" && call_number == 4 {
                let invalid = json!({
                    "scope_id":input["scope_id"], "keep_unit_ids":first, "additions":[],
                    "replace_parent":{"unit_id":input["parent_unit_id"],"kind":"state","retrieval_hint":"conflict"}, "omit_unit_ids":[]
                });
                return Json(
                    json!({"choices":[{"message":{"content":json!({"reviews":[invalid]}).to_string()}}]}),
                );
            }
            let review = match mode {
                "review-add.md" => json!({
                    "scope_id":input["scope_id"], "keep_unit_ids":first,
                    "additions":[{"unit_id":input["addable_unit_ids"][0],"kind":"procedure","retrieval_hint":"backup verification"}],
                    "replace_parent":null, "omit_unit_ids":[]
                }),
                "review-replace.md" => json!({
                    "scope_id":input["scope_id"], "keep_unit_ids":[], "additions":[],
                    "replace_parent":{"unit_id":input["parent_unit_id"],"kind":"state","retrieval_hint":"migration scope"}, "omit_unit_ids":[]
                }),
                "review-omit.md" => json!({
                    "scope_id":input["scope_id"], "keep_unit_ids":[], "additions":[],
                    "replace_parent":null, "omit_unit_ids":[selected]
                }),
                "review-invalid.md" => json!({
                    "scope_id":input["scope_id"], "keep_unit_ids":first,
                    "additions":[], "replace_parent":null, "omit_unit_ids":[]
                }),
                _ => unreachable!("unknown review test mode"),
            };
            return Json(
                json!({"choices":[{"message":{"content":json!({"reviews":[review]}).to_string()}}]}),
            );
        }
        return Json(json!({"choices":[{"message":{"content":json!({"reviews":[{
            "scope_id":input["scope_id"],
            "keep_unit_ids":input["first_selected"],
            "additions":[],
            "replace_parent":null,
            "omit_unit_ids":[]
        }]}).to_string()}}]}));
    }
    assert_eq!(
        request["response_format"]["json_schema"]["name"],
        "memory_unit_selection"
    );
    let input: Value =
        serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
    let source_path = input["source_path"].as_str();
    let selection_limit = if source_path == Some("profile-change.md") {
        match call_number {
            1 => None,
            2 | 3 => Some(1),
            _ => Some(0),
        }
    } else {
        None
    };
    let units = input["units"].as_array().unwrap();
    let selected_units = if source_path.is_some_and(|path| path.starts_with("review-")) {
        units
            .iter()
            .filter(|unit| {
                unit["heading_path"]
                    .as_array()
                    .is_some_and(|headings| headings.last().and_then(Value::as_str) == Some("执行"))
            })
            .collect::<Vec<_>>()
    } else {
        units.iter().collect::<Vec<_>>()
    };
    let selections = selected_units
        .into_iter()
        .take(selection_limit.unwrap_or(usize::MAX))
        .map(|unit| {
            json!({"unit_id":unit["unit_id"],"kind":"experience","retrieval_hint":"actual project experience"})
        })
        .collect::<Vec<_>>();
    Json(json!({"choices":[{"message":{"content":json!({"selections":selections}).to_string()}}]}))
}

async fn configure(
    state: &StateStore,
    context: &VaultContext,
    service: &MemoryService,
    calls: Arc<AtomicUsize>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(select_all))
                .with_state(calls),
        )
        .await
        .unwrap();
    });
    let providers = ProviderService::new(
        state.clone(),
        AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[23u8; 32]).unwrap(),
        ),
    );
    providers
        .set_provider_mode(context, ProviderMode::LocalOnly, None)
        .await
        .unwrap();
    let provider = providers
        .create_provider(ProviderInput {
            name: "selector".into(),
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
            external_model_id: "selector".into(),
            capabilities: ModelCapabilities {
                structured_output: true,
                max_output_tokens: Some(8192),
                ..Default::default()
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
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn explicit_memory_preserves_exact_body_and_rebuilds_without_legacy_reads() {
    let (_dir, state, context, core, service) = fixture().await;
    let body = "  Only in project A.\r\nFirst check; then execute.\r\n\n";
    let memory = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: body.into(),
                memory_type: Some(MemoryType::Procedure),
                idempotency_key: Some("operation".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    assert_eq!(memory.content, body);
    assert!(
        service
            .remember(
                &context,
                &core,
                RememberInput {
                    content: "only in project a.\nfirst check; then execute.".into(),
                    memory_type: Some(MemoryType::Procedure),
                    idempotency_key: Some("operation".into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert!(
        memory
            .canonical_path
            .as_ref()
            .unwrap()
            .as_str()
            .contains("memory-v3/")
    );
    let changed = service
        .update(
            &context,
            &core,
            memory.id,
            memory.revision,
            MemoryUpdateInput {
                content: Some("Otherwise stop.\n".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        service
            .update(
                &context,
                &core,
                memory.id,
                memory.revision,
                Default::default()
            )
            .await
            .is_err()
    );
    state
        .memory_units()
        .delete_explicit_projection(&context, changed.id, changed.revision)
        .await
        .unwrap();
    assert!(service.get(&context, changed.id).await.is_err());
    let rebuilt = service.rebuild(&context, &core).await.unwrap();
    assert_eq!(rebuilt.quarantined, 0);
    assert_eq!(
        service.get(&context, changed.id).await.unwrap().content,
        "Otherwise stop.\n"
    );
}

#[tokio::test]
async fn selection_checkpoints_publish_only_after_all_batches_and_survive_deletion_rebuild() {
    let (dir, state, context, core, service) = fixture().await;
    let calls = Arc::new(AtomicUsize::new(0));
    configure(&state, &context, &service, calls.clone()).await;
    let path = VaultPath::parse("practices.md").unwrap();
    let mut body = String::new();
    for i in 0..70 {
        body.push_str(&format!("# Case {i}\nOnly on host {i}.\n1. First check availability.\n2. Then move the model.\n3. Verify its device.\n\n"));
    }
    let file = core
        .create_bytes(
            &context,
            &path,
            body.as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    let first = service.extract_note(&context, &core, &path).await.unwrap();
    assert!(first.pending_batches > 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        state.memory_units().counts(&context).await.unwrap().total,
        0
    );
    // A new application instance resumes from persisted batches, not model state.
    let service = MemoryService::new(
        state.clone(),
        AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[23u8; 32]).unwrap(),
        ),
    );
    assert!(
        service
            .extract_note(&context, &core, &path)
            .await
            .unwrap()
            .pending_batches
            > 0
    );
    let mut final_batch = service.extract_note(&context, &core, &path).await.unwrap();
    while final_batch.items_published == 0 {
        final_batch = service.extract_note(&context, &core, &path).await.unwrap();
    }
    assert_eq!(final_batch.items_published, 70);
    assert_eq!(calls.load(Ordering::SeqCst), 6);
    let items = state
        .memory_units()
        .list(&context, &UnitFilter::default(), 100, 0)
        .await
        .unwrap();
    assert_eq!(items.len(), 70);
    for item in &items {
        assert!(body.contains(&item.content));
        assert!(item.content.contains("First check"));
        assert!(item.content.contains("Then move"));
    }
    let set = state
        .memory_units()
        .get_note_set_by_source(&context, file.id)
        .await
        .unwrap()
        .unwrap();
    let mut read = core
        .read_managed(&context, &set.canonical_path)
        .await
        .unwrap();
    let mut canonical = String::new();
    read.reader.read_to_string(&mut canonical).await.unwrap();
    assert!(canonical.contains("## 记忆"));
    assert!(canonical.contains("1. First check availability.\n2. Then move the model."));
    let other = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("other").unwrap(),
        dir.path().join("other"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&other, "other", VaultStatus::Active)
        .await
        .unwrap();
    assert!(service.get(&other, items[0].id).await.is_err());
    service
        .forget(&context, &core, items[0].id, items[0].revision)
        .await
        .unwrap();
    assert!(
        state
            .memory_units()
            .get_note_set_by_source(&context, file.id)
            .await
            .unwrap()
            .unwrap()
            .extraction_paused
    );
    assert!(
        service
            .extract_note(&context, &core, &path)
            .await
            .unwrap()
            .already_evaluated
    );
    assert_eq!(calls.load(Ordering::SeqCst), 6);
    let set = state
        .memory_units()
        .get_note_set_by_source(&context, file.id)
        .await
        .unwrap()
        .unwrap();
    state
        .memory_units()
        .delete_note_set_projection(&context, file.id, set.set_revision)
        .await
        .unwrap();
    let report = service.rebuild(&context, &core).await.unwrap();
    assert_eq!(report.quarantined, 0, "{:?}", report.quarantine_reasons);
    assert_eq!(
        state.memory_units().counts(&context).await.unwrap().total,
        69
    );
    assert!(
        state
            .memory_units()
            .get_note_set_by_source(&context, file.id)
            .await
            .unwrap()
            .unwrap()
            .extraction_paused
    );
    assert_eq!(calls.load(Ordering::SeqCst), 6);
}

#[tokio::test]
async fn extraction_profile_change_rechecks_unfinished_and_published_sets_atomically() {
    let (_dir, state, context, core, service) = fixture().await;
    let calls = Arc::new(AtomicUsize::new(0));
    configure(&state, &context, &service, calls.clone()).await;
    let path = VaultPath::parse("profile-change.md").unwrap();
    let mut body = String::new();
    for i in 0..40 {
        body.push_str(&format!(
            "# Case {i}\nObserved and verified condition {i}.\n\n"
        ));
    }
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

    // The first profile has two batches. Change it while the first profile's
    // first batch is only a checkpoint: the old checkpoint must not satisfy
    // the new input identity.
    assert!(
        service
            .extract_note(&context, &core, &path)
            .await
            .unwrap()
            .pending_batches
            > 0
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    service
        .set_extraction_policy(
            &context,
            ExtractionPolicy {
                enabled: true,
                request_timeout_seconds: 301,
            },
            None,
            None,
        )
        .await
        .unwrap();
    assert!(
        service
            .extract_note(&context, &core, &path)
            .await
            .unwrap()
            .pending_batches
            > 0
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        state.memory_units().counts(&context).await.unwrap().total,
        0
    );
    assert_eq!(
        service
            .extract_note(&context, &core, &path)
            .await
            .unwrap()
            .items_published,
        0
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let progress = state
        .memory_units()
        .selection_progress(&context)
        .await
        .unwrap();
    let progress = progress
        .iter()
        .find(|item| item["path"] == "profile-change.md")
        .expect("profile progress");
    assert!(
        progress["completed_batches"].as_i64().unwrap()
            < progress["total_batches"].as_i64().unwrap()
    );
    assert_eq!(
        service
            .extract_note(&context, &core, &path)
            .await
            .unwrap()
            .items_published,
        0
    );
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!(
        service
            .extract_note(&context, &core, &path)
            .await
            .unwrap()
            .items_published,
        2
    );
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    let old_items = state
        .memory_units()
        .list(&context, &UnitFilter::default(), 100, 0)
        .await
        .unwrap();
    assert_eq!(old_items.len(), 2);
    let old_id = old_items[0].id;
    assert!(
        service
            .extract_note(&context, &core, &path)
            .await
            .unwrap()
            .already_evaluated
    );
    assert_eq!(calls.load(Ordering::SeqCst), 5);

    // A published set is also re-evaluated under a new profile. The old set
    // remains visible until all new batches finish, then is replaced once.
    service
        .set_extraction_policy(
            &context,
            ExtractionPolicy {
                enabled: true,
                request_timeout_seconds: 302,
            },
            None,
            None,
        )
        .await
        .unwrap();
    let first_new = service.extract_note(&context, &core, &path).await.unwrap();
    assert!(!first_new.already_evaluated);
    assert!(first_new.pending_batches > 0);
    assert_eq!(
        state.memory_units().counts(&context).await.unwrap().total,
        2
    );
    assert_eq!(calls.load(Ordering::SeqCst), 6);
    assert_eq!(
        service
            .extract_note(&context, &core, &path)
            .await
            .unwrap()
            .items_published,
        0
    );
    assert_eq!(calls.load(Ordering::SeqCst), 7);
    assert_eq!(
        state.memory_units().counts(&context).await.unwrap().total,
        0
    );
    assert!(service.get(&context, old_id).await.is_err());
    let recall = service
        .recall(
            &context,
            mcp_vault_memory::v3::RecallRequest {
                query: "Observed and verified condition".to_owned(),
                context: mcp_vault_memory::v3::RecallContext::default(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(recall.memories.is_empty());
    assert!(
        service
            .extract_note(&context, &core, &path)
            .await
            .unwrap()
            .already_evaluated
    );
    assert_eq!(calls.load(Ordering::SeqCst), 7);
}

#[tokio::test]
async fn recall_keeps_exact_units_scope_and_size_pointers_within_the_final_envelope() {
    use mcp_vault_memory::v3::{RecallContext, RecallRequest};
    let (_dir, _state, context, core, service) = fixture().await;
    let mut ids = Vec::new();
    for content in [
        "Alpha procedure: STOP before moving the model.\n",
        "Alpha procedure: stop before moving the model.\n",
        "Alpha procedure: STOP before moving the model.\n\n",
    ] {
        ids.push(
            service
                .remember(
                    &context,
                    &core,
                    RememberInput {
                        content: content.into(),
                        ..Default::default()
                    },
                )
                .await
                .unwrap()
                .memory
                .unwrap()
                .id,
        );
    }
    let result = service
        .recall(
            &context,
            RecallRequest {
                query: "Alpha procedure".into(),
                context: RecallContext {
                    paths: vec!["unrelated/project/".into()],
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(result.memories.len(), 3);
    for id in ids {
        assert!(result.memories.iter().any(|m| m.id == id));
    }
    let large = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: format!(
                    "Largeprocedure alpha\n{}",
                    "Keep the entire ordered operation.\n".repeat(900)
                ),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    let short = service
        .remember(
            &context,
            &core,
            RememberInput {
                content: "Largeprocedure alpha: check availability first.".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    let result = service
        .recall(
            &context,
            RecallRequest {
                query: "Largeprocedure alpha".into(),
                max_tokens: 1024,
                include_score_breakdown: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(result.memories.iter().any(|m| m.id == short.id));
    assert!(!result.memories.iter().any(|m| m.id == large.id));
    assert!(result.pointers.iter().any(|p| p.id == large.id));
    assert!(serde_json::to_vec(&result).unwrap().len().div_ceil(4) <= 1024);
    assert_eq!(
        service.get(&context, large.id).await.unwrap().content,
        large.content
    );
}

#[tokio::test]
async fn note_units_require_note_access_and_source_edits_invalidate_them_immediately() {
    use mcp_vault_memory::v3::{MemoryReadAccess, RecallRequest};
    let (_dir, state, context, core, service) = fixture().await;
    let calls = Arc::new(AtomicUsize::new(0));
    configure(&state, &context, &service, calls.clone()).await;
    let path = VaultPath::parse("gpu.md").unwrap();
    let file=core.create_bytes(&context,&path,b"# GPU protocol\nOur adopted GPU protocol requires checking availability before moving the model.\n",Actor::system(),SourcePlane::System,None).await.unwrap().file;
    service.extract_note(&context, &core, &path).await.unwrap();
    let derived = service
        .list(&context, vec![], None, None, None, 10, 0)
        .await
        .unwrap()
        .remove(0);
    assert!(
        service
            .get_with_access(&context, derived.id, MemoryReadAccess::ExplicitOnly)
            .await
            .is_err()
    );
    assert!(
        service
            .list_with_access(
                &context,
                vec![],
                None,
                None,
                None,
                10,
                0,
                None,
                MemoryReadAccess::ExplicitOnly
            )
            .await
            .unwrap()
            .is_empty()
    );
    let denied = service
        .recall(
            &context,
            RecallRequest {
                query: "GPU protocol".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        denied.memories.is_empty() && denied.pointers.is_empty() && denied.related_notes.is_empty()
    );
    assert_eq!(denied.candidate_memory_count, 0);
    assert_eq!(
        service
            .recall(
                &context,
                RecallRequest {
                    query: "GPU protocol".into(),
                    access: MemoryReadAccess::All,
                    ..Default::default()
                }
            )
            .await
            .unwrap()
            .memories
            .len(),
        1
    );
    core.replace_bytes(
        &context,
        &path,
        file.current_revision,
        b"# GPU protocol\nThe GPU protocol is obsolete; use the reviewed new deployment plan.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    assert!(service.get(&context, derived.id).await.is_err());
    assert!(
        service
            .recall(
                &context,
                RecallRequest {
                    query: "GPU protocol".into(),
                    access: MemoryReadAccess::All,
                    ..Default::default()
                }
            )
            .await
            .unwrap()
            .memories
            .is_empty()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn overview_is_separate_navigation_with_source_invalidation_and_no_online_generation() {
    use mcp_vault_memory::v3::{MemoryReadAccess, OverviewRequest};
    let (_dir, state, context, core, service) = fixture().await;
    let calls = Arc::new(AtomicUsize::new(0));
    configure(&state, &context, &service, calls.clone()).await;
    let path = VaultPath::parse("operating.md").unwrap();
    let file = core
        .create_bytes(
            &context,
            &path,
            b"# Procedure\nOur adopted procedure: verify the GPU before moving the model.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    service.extract_note(&context, &core, &path).await.unwrap();
    let request = OverviewRequest {
        access: MemoryReadAccess::All,
        ..Default::default()
    };
    let initial = service
        .get_memory_overview(&context, request.clone())
        .await
        .unwrap();
    assert_eq!(initial.navigation_kind, "deterministic_navigation");
    assert_eq!(initial.entries.len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        service
            .generate_memory_overview(&context, request.clone())
            .await
            .unwrap()
    );
    let generated = service
        .get_memory_overview(&context, request.clone())
        .await
        .unwrap();
    assert_eq!(generated.navigation_kind, "generated_navigation");
    assert_eq!(generated.generated_sections.len(), 1);
    assert!(
        !service
            .generate_memory_overview(&context, request.clone())
            .await
            .unwrap()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let limited = service
        .get_memory_overview(&context, OverviewRequest::default())
        .await
        .unwrap();
    assert!(limited.entries.is_empty());
    assert!(limited.generated_sections.is_empty());
    core.replace_bytes(
        &context,
        &path,
        file.current_revision,
        b"# Procedure\nStop using the old procedure.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let invalidated = service
        .get_memory_overview(&context, request)
        .await
        .unwrap();
    assert_eq!(invalidated.navigation_kind, "deterministic_navigation");
    assert!(invalidated.entries.is_empty());
    assert!(invalidated.generated_sections.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cold_rebuild_rejects_forged_source_coordinates_and_restores_pause_after_source_edit() {
    let (_dir, state, context, core, service) = fixture().await;
    let calls = Arc::new(AtomicUsize::new(0));
    configure(&state, &context, &service, calls.clone()).await;
    let path = VaultPath::parse("scope.md").unwrap();
    let file=core.create_bytes(&context,&path,b"# Project Alpha\nOur adopted deployment procedure runs only in Alpha.\n\n# Project Beta\nBeta keeps the existing deployment procedure.\n",Actor::system(),SourcePlane::System,None).await.unwrap().file;
    for _ in 0..4 {
        if state
            .memory_units()
            .get_note_set_by_source(&context, file.id)
            .await
            .unwrap()
            .is_some()
        {
            break;
        }
        service.extract_note(&context, &core, &path).await.unwrap();
    }
    let set = state
        .memory_units()
        .get_note_set_by_source(&context, file.id)
        .await
        .unwrap()
        .unwrap();
    let mut read = core
        .read_managed(&context, &set.canonical_path)
        .await
        .unwrap();
    let mut canonical = String::new();
    read.reader.read_to_string(&mut canonical).await.unwrap();
    let items_line = canonical
        .lines()
        .find(|line| line.starts_with("items: "))
        .unwrap();
    let yaml = yaml_rust::YamlLoader::load_from_str(items_line).unwrap();
    let mut items: Value = serde_json::from_str(yaml[0]["items"].as_str().unwrap()).unwrap();
    items[0]["metadata"]["source_unit"]["headings"] = json!(["Project Beta"]);
    let forged = canonical.replacen(
        items_line,
        &format!(
            "items: {}",
            serde_json::to_string(&items.to_string()).unwrap()
        ),
        1,
    );
    let mutation = core
        .replace_managed_bytes(
            &context,
            &set.canonical_path,
            set.canonical_revision,
            forged.as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    state
        .memory_units()
        .delete_note_set_projection(&context, file.id, set.set_revision)
        .await
        .unwrap();
    let rejected = service.rebuild(&context, &core).await.unwrap();
    assert_eq!(
        rejected
            .quarantine_reasons
            .get("source_units_do_not_match_original"),
        Some(&1)
    );
    assert_eq!(
        state.memory_units().counts(&context).await.unwrap().total,
        0
    );
    core.replace_managed_bytes(
        &context,
        &set.canonical_path,
        mutation.file.current_revision,
        canonical.as_bytes(),
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        service.rebuild(&context, &core).await.unwrap().quarantined,
        0
    );
    let item = service
        .list(&context, vec![], None, None, None, 10, 0)
        .await
        .unwrap()
        .remove(0);
    service
        .forget(&context, &core, item.id, item.revision)
        .await
        .unwrap();
    core.replace_bytes(
        &context,
        &path,
        file.current_revision,
        b"# New source\nThe prior procedures have changed.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let paused = state
        .memory_units()
        .get_note_set_by_source(&context, file.id)
        .await
        .unwrap()
        .unwrap();
    state
        .memory_units()
        .delete_note_set_projection(&context, file.id, paused.set_revision)
        .await
        .unwrap();
    assert_eq!(
        service.rebuild(&context, &core).await.unwrap().quarantined,
        0
    );
    assert_eq!(
        state.memory_units().counts(&context).await.unwrap().total,
        0
    );
    assert!(
        state
            .memory_units()
            .get_note_set_by_source(&context, file.id)
            .await
            .unwrap()
            .unwrap()
            .extraction_paused
    );
    assert!(
        service
            .extract_note(&context, &core, &path)
            .await
            .unwrap()
            .already_evaluated
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn rebuild_accepts_published_minimal_parent_introduction_units() {
    let (_dir, state, context, core, service) = fixture().await;
    let calls = Arc::new(AtomicUsize::new(0));
    configure(&state, &context, &service, calls).await;
    let path = VaultPath::parse("intro.md").unwrap();
    let file = core
        .create_bytes(
            &context,
            &path,
            b"# Project\nOnly Alpha uses this procedure.\n## Procedure\nRun the verified steps.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    let mut result = service.extract_note(&context, &core, &path).await.unwrap();
    for _ in 0..4 {
        if result.items_published > 0 {
            break;
        }
        result = service.extract_note(&context, &core, &path).await.unwrap();
    }
    assert!(result.items_published > 0);
    let items = state
        .memory_units()
        .list(&context, &UnitFilter::default(), 100, 0)
        .await
        .unwrap();
    let intro = items
        .iter()
        .find(|item| {
            item.metadata["source_unit"]["body"]["text"]
                == json!("# Project\nOnly Alpha uses this procedure.\n")
        })
        .expect("published parent introduction");
    let intro_id = intro.id;
    let intro_content = intro.content.clone();
    let intro_source_unit = intro.metadata["source_unit"].clone();
    let set = state
        .memory_units()
        .get_note_set_by_source(&context, file.id)
        .await
        .unwrap()
        .unwrap();
    state
        .memory_units()
        .delete_note_set_projection(&context, file.id, set.set_revision)
        .await
        .unwrap();
    let report = service.rebuild(&context, &core).await.unwrap();
    assert_eq!(report.quarantined, 0, "{:?}", report.quarantine_reasons);
    let rebuilt = state
        .memory_units()
        .list(&context, &UnitFilter::default(), 100, 0)
        .await
        .unwrap();
    let restored_intro = rebuilt
        .iter()
        .find(|item| item.id == intro_id)
        .expect("restored intro");
    assert_eq!(restored_intro.content, intro_content);
    assert_eq!(restored_intro.metadata["source_unit"], intro_source_unit);
}

#[tokio::test]
async fn review_actions_add_replace_and_omit_publish_only_source_owned_units() {
    for mode in ["review-add.md", "review-replace.md", "review-omit.md"] {
        let (_dir, state, context, core, service) = fixture().await;
        let calls = Arc::new(AtomicUsize::new(0));
        configure(&state, &context, &service, calls.clone()).await;
        let path = VaultPath::parse(mode).unwrap();
        let source = "# 迁移\n## 备份\n### 校验\n迁移前必须验证备份可恢复。\n## 执行\n执行迁移。\n";
        let file = core
            .create_bytes(
                &context,
                &path,
                source.as_bytes(),
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await
            .unwrap()
            .file;
        let first = service.extract_note(&context, &core, &path).await.unwrap();
        assert!(first.pending_batches > 0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let progress = state
            .memory_units()
            .selection_progress(&context)
            .await
            .unwrap()
            .into_iter()
            .find(|item| item["path"] == mode)
            .unwrap();
        assert!(
            progress["completed_batches"].as_i64().unwrap()
                < progress["total_batches"].as_i64().unwrap()
        );
        let mut result = first;
        for _ in 0..5 {
            let before_calls = calls.load(Ordering::SeqCst);
            if result.items_published > 0 || result.already_evaluated {
                break;
            }
            result = service.extract_note(&context, &core, &path).await.unwrap();
            assert!(calls.load(Ordering::SeqCst) <= before_calls + 1);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let items = state
            .memory_units()
            .list(&context, &UnitFilter::default(), 100, 0)
            .await
            .unwrap();
        match mode {
            "review-add.md" => {
                let added = items
                    .iter()
                    .find(|item| item.content.contains("迁移前必须验证备份可恢复"))
                    .expect("added sibling");
                assert!(
                    added.metadata["source_unit"]["body"]["text"]
                        .as_str()
                        .unwrap()
                        .contains("迁移前必须验证备份可恢复")
                );
                assert!(items.iter().any(|item| item.content.contains("执行迁移")));
            }
            "review-replace.md" => {
                assert_eq!(items.len(), 1);
                assert!(items[0].content.contains("迁移前必须验证备份可恢复"));
                assert!(items[0].content.contains("执行迁移"));
                assert_eq!(
                    items[0].metadata["source_unit"]["headings"],
                    json!(["迁移"])
                );
                assert_eq!(items[0].kind.as_deref(), Some("state"));
                assert_eq!(items[0].metadata["retrieval_hint"], "migration scope");
            }
            "review-omit.md" => assert!(items.is_empty()),
            _ => unreachable!(),
        }
        let mut read = core.read(&context, &path).await.unwrap();
        let mut actual = String::new();
        read.reader.read_to_string(&mut actual).await.unwrap();
        assert_eq!(actual, source);
        assert!(
            state
                .memory_units()
                .get_note_set_by_source(&context, file.id)
                .await
                .unwrap()
                .is_some()
        );
    }
}

#[tokio::test]
async fn invalid_review_is_not_cached_and_resume_reuses_selection_checkpoint() {
    let (_dir, state, context, core, service) = fixture().await;
    let calls = Arc::new(AtomicUsize::new(0));
    configure(&state, &context, &service, calls.clone()).await;
    let path = VaultPath::parse("review-invalid.md").unwrap();
    let source = "# 迁移\n## 备份\n### 校验\n迁移前必须验证备份可恢复。\n## 执行\n执行迁移。\n";
    let file = core
        .create_bytes(
            &context,
            &path,
            source.as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    let mut initial = service.extract_note(&context, &core, &path).await.unwrap();
    for _ in 0..3 {
        if initial.items_published > 0 {
            break;
        }
        initial = service.extract_note(&context, &core, &path).await.unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let set_one = state
        .memory_units()
        .get_note_set_by_source(&context, file.id)
        .await
        .unwrap()
        .unwrap();
    let old_items = state
        .memory_units()
        .list_note_set_items(&context, set_one.id)
        .await
        .unwrap();
    let old_fingerprint = old_items
        .iter()
        .map(|item| {
            (
                item.memory.id,
                item.memory.content.clone(),
                item.memory.metadata.clone(),
            )
        })
        .collect::<Vec<_>>();
    let error = service
        .extract_note_with_options(
            &context,
            &core,
            &path,
            NoteExtractionOptions {
                include_evaluated: true,
            },
        )
        .await;
    assert!(error.is_ok());
    let error = service
        .extract_note_with_options(
            &context,
            &core,
            &path,
            NoteExtractionOptions {
                include_evaluated: true,
            },
        )
        .await;
    assert!(error.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!(
        state
            .memory_units()
            .get_note_set_by_source(&context, file.id)
            .await
            .unwrap(),
        Some(set_one.clone())
    );
    let unchanged = state
        .memory_units()
        .list_note_set_items(&context, set_one.id)
        .await
        .unwrap()
        .iter()
        .map(|item| {
            (
                item.memory.id,
                item.memory.content.clone(),
                item.memory.metadata.clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(unchanged, old_fingerprint);
    let service = MemoryService::new(
        state.clone(),
        AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[23u8; 32]).unwrap(),
        ),
    );
    let mut result = service
        .extract_note_with_options(
            &context,
            &core,
            &path,
            NoteExtractionOptions {
                include_evaluated: true,
            },
        )
        .await
        .unwrap();
    for _ in 0..3 {
        if result.items_published > 0 {
            break;
        }
        result = service
            .extract_note_with_options(
                &context,
                &core,
                &path,
                NoteExtractionOptions {
                    include_evaluated: true,
                },
            )
            .await
            .unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    let set_two = state
        .memory_units()
        .get_note_set_by_source(&context, file.id)
        .await
        .unwrap()
        .unwrap();
    assert!(set_two.set_revision > set_one.set_revision);
    assert!(
        state
            .memory_units()
            .get_note_set_by_source(&context, file.id)
            .await
            .unwrap()
            .is_some()
    );
}
