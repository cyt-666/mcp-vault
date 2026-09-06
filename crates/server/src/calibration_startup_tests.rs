//! A01: exercise the same startup compensation loop and durable worker as run().
use crate::workers::{
    Cancellation, JobOutcome, WorkerConfig, WorkerSupervisor, retrieval_calibration_job_handler,
};
use axum::{Json, Router, extract::State as AxumState, routing::post};
use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_core::VaultCore;
use mcp_vault_domain::{MaintenanceGate, Revision, VaultContext, VaultId, VaultSlug};
use mcp_vault_memory::{MemoryService, RecallRequest, RememberInput};
use mcp_vault_providers::{
    EmbeddingSourceRef, ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind,
    ProviderMode, ProviderService, ProviderSettings,
};
use mcp_vault_state::{StateStore, VaultStatus};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::time::sleep;

async fn synthetic_embeddings(
    AxumState(calls): AxumState<Arc<AtomicUsize>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    calls.fetch_add(1, Ordering::SeqCst);
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/memory-quality/calibration.json"
    ))
    .unwrap();
    let documents = corpus["documents"].as_array().unwrap();
    let queries = corpus["queries"].as_array().unwrap();
    Json(
        json!({"data":body["input"].as_array().unwrap().iter().enumerate().map(|(index,input)|{
        let text=input.as_str().unwrap();
        let document=documents.iter().position(|doc|text.to_lowercase().contains(&doc["content"].as_str().unwrap().to_lowercase()));
        let query=queries.iter().find(|query|query["query"].as_str()==Some(text));
        let coordinate=document.or_else(||query.and_then(|query|query["relevant"][0].as_str()).and_then(|id|documents.iter().position(|doc|doc["id"].as_str()==Some(id)))).unwrap_or(if query.is_some(){63}else{60});
        let mut vector=vec![0.0_f32;64];vector[coordinate]=1.0;
        json!({"index":index,"embedding":vector})
    }).collect::<Vec<_>>()}),
    )
}

fn providers(state: &StateStore) -> ProviderService {
    ProviderService::new(
        state.clone(),
        AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[37; 32]).unwrap(),
        ),
    )
}

#[tokio::test]
async fn obsolete_calibration_job_retires_only_its_vaults_unfinished_profiles() {
    let dir = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let mut contexts = Vec::new();
    for slug in ["obsolete", "isolated"] {
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new(slug).unwrap(),
            dir.path().join(slug),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, slug, VaultStatus::Active)
            .await
            .unwrap();
        for channel in ["memory", "note"] {
            state
                .calibrations()
                .ensure(&context, channel, "old-profile")
                .await
                .unwrap();
            state
                .calibrations()
                .reserve_request(&context, channel, "old-profile", 123, 900_000)
                .await
                .unwrap();
            state
                .calibrations()
                .checkpoint(
                    &context,
                    channel,
                    "old-profile",
                    &json!({"saved":[1.0,0.0]}),
                )
                .await
                .unwrap();
        }
        contexts.push(context);
    }
    let context = &contexts[0];
    let provider_service = providers(&state);
    let provider = provider_service
        .create_provider(ProviderInput {
            name: "Never contacted".into(),
            kind: ProviderKind::EmbeddingHttp,
            base_url: "http://127.0.0.1:1/v1/".parse().unwrap(),
            settings: ProviderSettings::default(),
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();
    let model = provider_service
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "new-model".into(),
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
    provider_service
        .bind_model(Some(context), "embedding_memory", model.id, json!({}), None)
        .await
        .unwrap();
    let memory = MemoryService::with_provider_service(state.clone(), provider_service);
    let current = memory
        .calibration_profile(context, "memory")
        .await
        .unwrap()
        .unwrap();
    state
        .calibrations()
        .ensure(context, "memory", &current.signature)
        .await
        .unwrap();
    state
        .calibrations()
        .ensure(context, "memory", "completed-profile")
        .await
        .unwrap();
    state
        .calibrations()
        .finish(
            context,
            "memory",
            "completed-profile",
            "passed",
            &json!({"kept":true}),
        )
        .await
        .unwrap();
    let job = state
        .jobs()
        .enqueue(
            context,
            "retrieval.calibrate",
            "obsolete-test",
            &json!({"signatures":["old-profile"],"explicit_diagnostic":true}),
            0,
            3,
            0,
        )
        .await
        .unwrap();
    let handler = retrieval_calibration_job_handler(state.clone(), memory);
    assert_eq!(
        handler(job, Cancellation::default()).await,
        JobOutcome::Complete
    );
    for channel in ["memory", "note"] {
        let old = state
            .calibrations()
            .get(context, channel, "old-profile")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            old.status, "cancelled",
            "a skipped obsolete job cannot leave a running report"
        );
        assert_eq!(old.requests, 1);
        assert_eq!(old.request_bytes, 123);
        assert_eq!(old.checkpoint_json, json!({"saved":[1.0,0.0]}).to_string());
        assert_eq!(
            serde_json::from_str::<Value>(old.report_json.as_deref().unwrap()).unwrap()["error_code"],
            "calibration_profile_changed"
        );
        assert_eq!(
            state
                .calibrations()
                .get(&contexts[1], channel, "old-profile")
                .await
                .unwrap()
                .unwrap()
                .status,
            "running"
        );
    }
    let pending = state
        .calibrations()
        .get(context, "memory", &current.signature)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.status, "pending");
    assert_eq!(pending.requests, 0);
    assert_eq!(
        state
            .calibrations()
            .get(context, "memory", "completed-profile")
            .await
            .unwrap()
            .unwrap()
            .status,
        "passed"
    );
}

#[tokio::test]
async fn a01_existing_vectors_work_on_start_without_benchmark_or_regeneration() {
    let dir = tempfile::tempdir().unwrap();
    let database = format!("sqlite://{}", dir.path().join("state.sqlite").display());
    let state = StateStore::connect_and_migrate(&database).await.unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("a01").unwrap(),
        dir.path().join("vault"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&context, "A01 upgrade fixture", VaultStatus::Active)
        .await
        .unwrap();
    let core = VaultCore::new(
        state.clone(),
        dir.path().join("history"),
        Default::default(),
        Default::default(),
        Default::default(),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fake = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/v1/embeddings", post(synthetic_embeddings))
                .with_state(counter),
        )
        .await
        .unwrap();
    });
    let provider_service = providers(&state);
    provider_service
        .set_provider_mode(&context, ProviderMode::LocalOnly, None)
        .await
        .unwrap();
    let provider = provider_service
        .create_provider(ProviderInput {
            name: "isolated A01 contract endpoint".into(),
            kind: ProviderKind::EmbeddingHttp,
            base_url: url::Url::parse(&format!("http://{address}/v1/")).unwrap(),
            settings: ProviderSettings::default(),
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();
    let model = provider_service
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "synthetic-contract-only".into(),
            capabilities: ModelCapabilities {
                embeddings: true,
                dimension: Some(64),
                ..Default::default()
            },
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await
        .unwrap();
    provider_service
        .bind_model(
            Some(&context),
            "embedding_memory",
            model.id,
            json!({}),
            None,
        )
        .await
        .unwrap();
    let memory = MemoryService::with_provider_service(state.clone(), provider_service);
    let original = memory
        .remember(
            &context,
            &core,
            RememberInput {
                content: "The existing orchard assertion is already materialized.".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    let bundle = state
        .current_memory()
        .get(&context, original.id)
        .await
        .unwrap()
        .unwrap();
    memory
        .reembed_sources(
            &context,
            model.id,
            &[EmbeddingSourceRef {
                object_type: "memory".into(),
                object_id: original.id.to_string(),
                chunk_key: "body-v3:0000".into(),
                content_hash: bundle.memory.content_hash,
            }],
        )
        .await
        .unwrap();
    let vectors = state
        .providers()
        .list_embeddings(&context, model.id, "memory", 100, 0)
        .await
        .unwrap();
    assert!(!vectors.is_empty());
    assert!(
        !memory
            .calibration_status(&context, "memory")
            .await
            .unwrap()
            .active
    );
    // New application services read the already populated disk database. No
    // bind event, extraction, browser, run endpoint or calibration PUT is used.
    let upgraded = StateStore::connect_and_migrate(&database).await.unwrap();
    let memory = MemoryService::with_provider_service(upgraded.clone(), providers(&upgraded));
    for restart in 0..2 {
        let before = calls.load(Ordering::SeqCst);
        let supervisor = WorkerSupervisor::new(
            upgraded.clone(),
            Arc::new(|_| Box::pin(async { Ok(()) })),
            WorkerConfig {
                poll_interval: Duration::from_millis(5),
                ..Default::default()
            },
        )
        .unwrap();
        supervisor
            .register_job_handler(
                "retrieval.calibrate",
                retrieval_calibration_job_handler(upgraded.clone(), memory.clone()),
            )
            .unwrap();
        supervisor
            .register_job_handler(
                "vault.reconcile",
                Arc::new(|_, _| Box::pin(async { JobOutcome::Complete })),
            )
            .unwrap();
        supervisor
            .register_job_handler(
                "embedding.rebuild",
                crate::workers::embedding_job_handler(
                    upgraded.clone(),
                    mcp_vault_indexer::IndexService::with_provider_service(
                        upgraded.clone(),
                        providers(&upgraded),
                    ),
                    memory.clone(),
                ),
            )
            .unwrap();
        let stop = Cancellation::default();
        let run = tokio::spawn({
            let worker = supervisor.clone();
            let stop = stop.clone();
            async move { worker.run(stop).await }
        });
        supervisor.wait_until_running().await;
        let tick_stop = Cancellation::default();
        let tick = tokio::spawn(super::run_reconciliation_loop(
            upgraded.clone(),
            memory.clone(),
            Duration::from_secs(3600),
            MaintenanceGate::new(),
            tick_stop.clone(),
        ));
        sleep(Duration::from_millis(250)).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            before,
            "startup must not dispatch synthetic inputs"
        );
        assert!(
            memory
                .calibration_status(&context, "memory")
                .await
                .unwrap()
                .run
                .is_none()
        );
        if restart == 0 {
            assert_eq!(calls.load(Ordering::SeqCst), before);
            let result = memory
                .recall(
                    &context,
                    RecallRequest {
                        query: "完全无字面重合的查询".into(),
                        include_related_notes: false,
                        include_score_breakdown: true,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            assert_eq!(result.memories[0].id, original.id);
            assert!(
                result.memories[0]
                    .score_breakdown
                    .as_ref()
                    .unwrap()
                    .contains_key("semantic_cosine")
            );
        }
        let completed_calls = calls.load(Ordering::SeqCst);
        sleep(Duration::from_millis(100)).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            completed_calls,
            "an applicable report must not schedule another billable calibration"
        );
        tick_stop.cancel();
        tick.await.unwrap();
        stop.cancel();
        run.await.unwrap();
        assert_eq!(
            state
                .providers()
                .list_embeddings(&context, model.id, "memory", 100, 0)
                .await
                .unwrap(),
            vectors,
            "business vectors must remain byte-identical"
        );
        let current = memory.get(&context, original.id).await.unwrap();
        assert_eq!(current.content, original.content);
        assert_eq!(current.revision, original.revision);
        assert_eq!(current.canonical_revision, original.canonical_revision);
    }
    assert_eq!(
        state
            .jobs()
            .list(&context, None, Some("retrieval.calibrate"), 100, 0)
            .await
            .unwrap()
            .len(),
        0
    );
    fake.abort();
}
