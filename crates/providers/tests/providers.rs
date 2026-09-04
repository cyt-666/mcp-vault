use std::{
    convert::Infallible,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use mcp_vault_auth::{AuthService, MasterKeyRing, SecretString};
use mcp_vault_domain::{DomainError, Revision, VaultContext, VaultId, VaultSlug};
use mcp_vault_providers::{
    AuthStyle, EmbeddingInput, EmbeddingRequest, EmbeddingSourceRef, EmbeddingSourceResolver,
    ModelCapabilities, ModelInput, ModelSettings, OpenAiStructuredOutputMode, ProviderError,
    ProviderInput, ProviderKind, ProviderMode, ProviderService, ProviderSettings,
    ProviderTransport, RequestOptions, StructuredGenerationRequest, endpoint_url,
    validate_endpoint,
};
use mcp_vault_state::{StateError, StateStore, VaultStatus};
use reqwest::Method;
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::net::TcpListener;
use url::Url;

async fn fake_chat(Json(request): Json<Value>) -> Response {
    if request["response_format"]["type"] != "json_schema"
        || request.get("max_tokens").is_none()
        || request.get("max_completion_tokens").is_some()
    {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }
    Json(json!({
        "id": "fake-response",
        "model": "fake-chat",
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "{\"answer\":\"ok\"}"
            }
        }],
        "usage": {"total_tokens": 3}
    }))
    .into_response()
}

async fn capture_vendor_chat(
    State(captured): State<Arc<Mutex<Vec<Value>>>>,
    Json(request): Json<Value>,
) -> Json<Value> {
    captured.lock().unwrap().push(request);
    Json(json!({
        "id": "fake-mimo-response",
        "model": "mimo-v2.5",
        "choices": [{
            "finish_reason": "stop",
            "message": {"role": "assistant", "content": "{\"answer\":\"ok\"}"}
        }]
    }))
}

async fn fake_embeddings(Json(request): Json<Value>) -> Json<Value> {
    let count = request
        .get("input")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let data = (0..count)
        .map(|index| {
            json!({
                "index": index,
                "embedding": if index == 0 {
                    json!([1.0, 0.0, 0.0])
                } else {
                    json!([0.0, 1.0, 0.0])
                }
            })
        })
        .collect::<Vec<_>>();
    Json(json!({
        "model": "fake-embed",
        "data": data,
        "usage": {"prompt_tokens": count, "total_tokens": count}
    }))
}

async fn fake_models() -> Json<serde_json::Value> {
    Json(json!({
        "data": [
            {"id": "fake-chat"},
            {"id": "fake-embed"}
        ]
    }))
}

async fn fake_server() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route("/v1/chat/completions", post(fake_chat))
        .route("/v1/embeddings", post(fake_embeddings))
        .route("/v1/models", get(fake_models));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (address, task)
}

struct TestResolver;

#[async_trait]
impl EmbeddingSourceResolver for TestResolver {
    async fn resolve_source(
        &self,
        _context: &VaultContext,
        _source: &EmbeddingSourceRef,
    ) -> Result<Option<String>, ProviderError> {
        Ok(Some("reembedded source".to_owned()))
    }
}

async fn transient_response(State(attempts): State<Arc<AtomicUsize>>) -> impl IntoResponse {
    if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"retry": true})),
        )
            .into_response()
    } else {
        Json(json!({"ok": true})).into_response()
    }
}

async fn unauthorized_response() -> impl IntoResponse {
    StatusCode::UNAUTHORIZED
}

async fn invalid_content_response() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain")],
        "{\"ok\":true}",
    )
}

async fn invalid_json_response() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        "not-json",
    )
}

async fn redirect_response() -> impl IntoResponse {
    (StatusCode::FOUND, [(header::LOCATION, "/retry")], "")
}

async fn slow_response() -> impl IntoResponse {
    tokio::time::sleep(Duration::from_millis(100)).await;
    Json(json!({"ok": true}))
}

async fn delayed_body_response(State(attempts): State<Arc<AtomicUsize>>) -> Response {
    attempts.fetch_add(1, Ordering::SeqCst);
    let stream = futures_util::stream::once(async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok::<Bytes, Infallible>(Bytes::from_static(b"{\"ok\":true}"))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn interrupted_body_response(State(attempts): State<Arc<AtomicUsize>>) -> Response {
    attempts.fetch_add(1, Ordering::SeqCst);
    let stream = futures_util::stream::unfold(0_u8, |state| async move {
        match state {
            0 => Some((Ok(Bytes::from_static(b"{\"ok\":")), 1)),
            1 => {
                tokio::time::sleep(Duration::from_millis(25)).await;
                Some((
                    Err::<Bytes, std::io::Error>(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "fixture response interrupted",
                    )),
                    2,
                ))
            }
            _ => None,
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from_stream(stream))
        .unwrap()
}

#[derive(Default)]
struct ConcurrencyState {
    active: AtomicUsize,
    maximum: AtomicUsize,
}

async fn concurrency_models(State(state): State<Arc<ConcurrencyState>>) -> Json<Value> {
    let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
    state.maximum.fetch_max(active, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(40)).await;
    state.active.fetch_sub(1, Ordering::SeqCst);
    Json(json!({"data": [{"id": "shared-model"}]}))
}

async fn context(state: &StateStore, slug: &str, root: PathBuf) -> VaultContext {
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new(slug).unwrap(),
        root,
        Revision::new(1),
    )
    .unwrap();
    state
        .vaults()
        .insert(&context, slug, VaultStatus::Active)
        .await
        .unwrap();
    context
}

#[tokio::test]
async fn provider_service_uses_encrypted_secrets_and_vault_model_bindings() {
    let (address, server) = fake_server().await;
    let directory = tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let work = context(&state, "work", directory.path().join("work")).await;
    let other = context(&state, "other", directory.path().join("other")).await;
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[4_u8; 32]).unwrap(),
    );
    let service = ProviderService::new(state.clone(), auth.clone());
    service
        .set_provider_mode(&work, ProviderMode::LocalOnly, None)
        .await
        .unwrap();
    let provider = service
        .create_provider(ProviderInput {
            name: "local fake".to_owned(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: Url::parse(&format!("http://{address}/v1/")).unwrap(),
            settings: ProviderSettings::default(),
            enabled: true,
            secret: Some(SecretString::new("fake-secret")),
        })
        .await
        .unwrap();
    let secret = state
        .auth()
        .get_secret(provider.secret_id.unwrap())
        .await
        .unwrap();
    assert!(!format!("{secret:?}").contains("fake-secret"));

    let discovered = service.test_provider(&work, provider.id).await.unwrap();
    let chat = discovered
        .iter()
        .find(|model| model.external_model_id == "fake-chat")
        .unwrap();
    let embedding = discovered
        .iter()
        .find(|model| model.external_model_id == "fake-embed")
        .unwrap();
    service
        .bind_model(Some(&work), "note_summary", chat.id, json!({}), None)
        .await
        .unwrap();
    let structured = service
        .generate_for_role(
            &work,
            "note_summary",
            &StructuredGenerationRequest {
                model: "fake-chat".to_owned(),
                system: "Return the requested object.".to_owned(),
                user: "untrusted note text".to_owned(),
                schema_name: "answer".to_owned(),
                schema: json!({
                    "type": "object",
                    "properties": {"answer": {"type": "string"}},
                    "required": ["answer"],
                    "additionalProperties": false
                }),
                missing_required_string_fallbacks: Vec::new(),
                max_output_tokens: 32,
                temperature: Some(0.0),
                timeout: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(structured.value["answer"], "ok");
    let invalid_schema = service
        .generate_structured(
            &work,
            chat.id,
            &StructuredGenerationRequest {
                model: "fake-chat".to_owned(),
                system: "Return the requested object.".to_owned(),
                user: "untrusted note text".to_owned(),
                schema_name: "wrong".to_owned(),
                schema: json!({
                    "type": "object",
                    "properties": {"answer": {"type": "integer"}},
                    "required": ["answer"],
                    "additionalProperties": false
                }),
                missing_required_string_fallbacks: Vec::new(),
                max_output_tokens: 32,
                temperature: None,
                timeout: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        invalid_schema,
        ProviderError::SchemaValidation {
            issue: "type_mismatch",
            ref path,
        } if path == "$.answer"
    ));

    let embeddings = service.embeddings();
    let records = embeddings
        .embed_and_store(
            &work,
            embedding.id,
            &[
                EmbeddingInput {
                    source: EmbeddingSourceRef {
                        object_type: "note".to_owned(),
                        object_id: "note-a".to_owned(),
                        chunk_key: "root".to_owned(),
                        content_hash: "sha256:a".to_owned(),
                    },
                    text: "first".to_owned(),
                },
                EmbeddingInput {
                    source: EmbeddingSourceRef {
                        object_type: "note".to_owned(),
                        object_id: "note-b".to_owned(),
                        chunk_key: "root".to_owned(),
                        content_hash: "sha256:b".to_owned(),
                    },
                    text: "second".to_owned(),
                },
            ],
        )
        .await
        .unwrap();
    assert_eq!(records.len(), 2);
    embeddings
        .embed_and_store(
            &work,
            embedding.id,
            &[EmbeddingInput {
                source: EmbeddingSourceRef {
                    object_type: "note".to_owned(),
                    object_id: "note-a".to_owned(),
                    chunk_key: "appendix".to_owned(),
                    content_hash: "sha256:a-appendix".to_owned(),
                },
                text: "another first".to_owned(),
            }],
        )
        .await
        .unwrap();
    embeddings
        .embed_and_store(
            &work,
            embedding.id,
            &[EmbeddingInput {
                source: EmbeddingSourceRef {
                    object_type: "memory".to_owned(),
                    object_id: "memory-a".to_owned(),
                    chunk_key: "body".to_owned(),
                    content_hash: "sha256:memory-a".to_owned(),
                },
                text: "memory".to_owned(),
            }],
        )
        .await
        .unwrap();
    let hits = embeddings
        .search(&work, embedding.id, "note", &[1.0, 0.0, 0.0], 1)
        .await
        .unwrap();
    assert_eq!(hits[0].embedding.object_id, "note-a");
    assert_eq!(hits[0].score, 1.0);
    let tied_hits = embeddings
        .search(&work, embedding.id, "note", &[1.0, 0.0, 0.0], 3)
        .await
        .unwrap();
    assert_eq!(tied_hits[0].embedding.chunk_key, "appendix");
    assert_eq!(tied_hits[1].embedding.chunk_key, "root");
    assert_eq!(tied_hits[2].embedding.object_id, "note-b");
    let differently_ranked = embeddings
        .search(&work, embedding.id, "note", &[0.0, 1.0, 0.0], 1)
        .await
        .unwrap();
    assert_eq!(differently_ranked[0].embedding.object_id, "note-b");
    assert_eq!(differently_ranked[0].score, 1.0);
    let memory_hits = embeddings
        .search(&work, embedding.id, "memory", &[1.0, 0.0, 0.0], 1)
        .await
        .unwrap();
    assert_eq!(memory_hits[0].embedding.object_id, "memory-a");
    assert!(
        embeddings
            .search(&other, embedding.id, "note", &[1.0, 0.0, 0.0], 10)
            .await
            .unwrap()
            .is_empty()
    );
    let coverage = embeddings.coverage(&work, embedding.id).await.unwrap();
    assert_eq!(coverage.total, 4);
    assert_eq!(coverage.objects, 3);
    assert_eq!(coverage.dimensions, vec![3]);

    let changed_model = service
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "fake-embed-v2".to_owned(),
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
    let rebuilt = embeddings
        .reembed_with_resolver(
            &work,
            changed_model.id,
            &[EmbeddingSourceRef {
                object_type: "note".to_owned(),
                object_id: "note-a".to_owned(),
                chunk_key: "root".to_owned(),
                content_hash: "sha256:changed".to_owned(),
            }],
            &TestResolver,
        )
        .await
        .unwrap();
    assert_eq!(rebuilt.len(), 1);
    assert_eq!(
        embeddings
            .coverage(&work, changed_model.id)
            .await
            .unwrap()
            .total,
        1
    );

    let wrong_dimension = service
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "fake-wrong-dimension".to_owned(),
            capabilities: ModelCapabilities {
                embeddings: true,
                dimension: Some(2),
                ..ModelCapabilities::default()
            },
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await
        .unwrap();
    let dimension_error = service
        .embed(
            &work,
            wrong_dimension.id,
            &EmbeddingRequest {
                model: "fake-wrong-dimension".to_owned(),
                inputs: vec!["dimension mismatch".to_owned(), "second".to_owned()],
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(dimension_error, ProviderError::DimensionMismatch));
    let job = embeddings
        .schedule_reembedding(
            &work,
            embedding.id,
            &[EmbeddingSourceRef {
                object_type: "note".to_owned(),
                object_id: "note-a".to_owned(),
                chunk_key: "root".to_owned(),
                content_hash: "sha256:a".to_owned(),
            }],
        )
        .await
        .unwrap();
    assert_eq!(job.job_type, "embedding.rebuild");
    assert_eq!(job.payload["projection_version"], 2);
    assert!(job.dedup_key.contains(":embedding:v2:"));
    assert!(job.payload.to_string().contains("note-a"));
    assert!(!job.payload.to_string().contains("first"));

    server.abort();
}

#[tokio::test]
async fn provider_deletion_is_revision_checked_and_cleans_only_dependent_state() {
    let (address, server) = fake_server().await;
    let directory = tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let work = context(&state, "delete-work", directory.path().join("delete-work")).await;
    let other = context(
        &state,
        "delete-other",
        directory.path().join("delete-other"),
    )
    .await;
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[13_u8; 32]).unwrap(),
    );
    let service = ProviderService::new(state.clone(), auth);
    for context in [&work, &other] {
        service
            .set_provider_mode(context, ProviderMode::LocalOnly, None)
            .await
            .unwrap();
    }
    let provider = service
        .create_provider(ProviderInput {
            name: "provider to delete".to_owned(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: Url::parse(&format!("http://{address}/v1/")).unwrap(),
            settings: ProviderSettings::default(),
            enabled: true,
            secret: Some(SecretString::new("delete-secret")),
        })
        .await
        .unwrap();
    let unrelated = service
        .create_provider(ProviderInput {
            name: "provider to retain".to_owned(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: Url::parse(&format!("http://{address}/v1/")).unwrap(),
            settings: ProviderSettings::default(),
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();
    let original_secret_id = provider.secret_id.unwrap();
    let provider = service
        .update_provider(
            provider,
            ProviderInput {
                name: "provider to delete after edit".to_owned(),
                kind: ProviderKind::OpenAiCompatible,
                base_url: Url::parse(&format!("http://{address}/v1/")).unwrap(),
                settings: ProviderSettings::default(),
                enabled: true,
                secret: Some(SecretString::new("replacement-secret")),
            },
        )
        .await
        .unwrap();
    let secret_id = provider.secret_id.unwrap();
    assert_ne!(secret_id, original_secret_id);
    assert!(
        state
            .auth()
            .get_secret(original_secret_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(state.auth().count_encrypted_secrets().await.unwrap(), 1);
    let models = service.test_provider(&work, provider.id).await.unwrap();
    let chat = models
        .iter()
        .find(|model| model.external_model_id == "fake-chat")
        .unwrap();
    let embedding = models
        .iter()
        .find(|model| model.external_model_id == "fake-embed")
        .unwrap();
    service
        .bind_model(None, "note_summary", chat.id, json!({}), None)
        .await
        .unwrap();
    service
        .bind_model(
            Some(&other),
            "embedding_memory",
            embedding.id,
            json!({}),
            None,
        )
        .await
        .unwrap();
    let work_embedding = service
        .embeddings()
        .embed_and_store(
            &work,
            embedding.id,
            &[EmbeddingInput {
                source: EmbeddingSourceRef {
                    object_type: "note".to_owned(),
                    object_id: "work-note".to_owned(),
                    chunk_key: "root".to_owned(),
                    content_hash: "sha256:work".to_owned(),
                },
                text: "work text".to_owned(),
            }],
        )
        .await
        .unwrap()
        .remove(0);
    let other_embedding = service
        .embeddings()
        .embed_and_store(
            &other,
            embedding.id,
            &[EmbeddingInput {
                source: EmbeddingSourceRef {
                    object_type: "memory".to_owned(),
                    object_id: "other-memory".to_owned(),
                    chunk_key: "root".to_owned(),
                    content_hash: "sha256:other".to_owned(),
                },
                text: "other text".to_owned(),
            }],
        )
        .await
        .unwrap()
        .remove(0);

    let stale = service
        .delete_provider(provider.id, Some(Revision::new(99)))
        .await
        .unwrap_err();
    assert!(matches!(
        stale,
        ProviderError::State(StateError::InvalidDomain(
            DomainError::RevisionConflict { .. }
        ))
    ));
    assert!(service.get_provider(provider.id).await.is_ok());

    let summary = service
        .delete_provider(provider.id, Some(provider.revision))
        .await
        .unwrap();
    assert_eq!(summary.models_deleted, 2);
    assert_eq!(summary.bindings_deleted, 2);
    assert_eq!(summary.embeddings_deleted, 2);
    assert_eq!(summary.secrets_deleted, 1);
    assert!(matches!(
        service.get_provider(provider.id).await,
        Err(ProviderError::NotFound)
    ));
    assert_eq!(
        service.get_provider(unrelated.id).await.unwrap().id,
        unrelated.id
    );
    assert!(
        state
            .providers()
            .list_models(Some(provider.id), 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        state
            .providers()
            .get_binding(None, "note_summary")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        state
            .providers()
            .get_binding(Some(&other), "embedding_memory")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        state
            .providers()
            .get_embedding(&work, work_embedding.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        state
            .providers()
            .get_embedding(&other, other_embedding.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(state.auth().get_secret(secret_id).await.unwrap().is_none());
    let stale_update = service
        .update_provider(
            provider.clone(),
            ProviderInput {
                name: "must remain deleted".to_owned(),
                kind: ProviderKind::OpenAiCompatible,
                base_url: Url::parse(&format!("http://{address}/v1/")).unwrap(),
                settings: ProviderSettings::default(),
                enabled: true,
                secret: Some(SecretString::new("must-not-be-orphaned")),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(stale_update, ProviderError::NotFound));
    assert_eq!(state.auth().count_encrypted_secrets().await.unwrap(), 0);
    assert!(
        state
            .providers()
            .get_health(provider.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        state
            .integrity_check()
            .await
            .unwrap()
            .foreign_key_violations,
        0
    );

    server.abort();
}

#[tokio::test]
async fn provider_service_sends_first_class_vendor_structured_generation_contracts() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/v1/chat/completions", post(capture_vendor_chat))
        .with_state(captured.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let directory = tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let work = context(
        &state,
        "vendor-contracts",
        directory.path().join("vendor-contracts"),
    )
    .await;
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[9_u8; 32]).unwrap(),
    );
    let service = ProviderService::new(state, auth);
    service
        .set_provider_mode(&work, ProviderMode::LocalOnly, None)
        .await
        .unwrap();
    let cases = [
        (ProviderKind::DeepSeek, "deepseek-v4-pro"),
        (ProviderKind::XiaomiMimo, "mimo-v2.5"),
        (ProviderKind::ZhipuGlm, "glm-5.2"),
        (ProviderKind::MoonshotKimi, "kimi-k2.6"),
        (ProviderKind::GoogleGemini, "gemini-3.7-flash"),
        (ProviderKind::AlibabaQwen, "qwen3.8-max"),
    ];
    for (kind, external_model_id) in cases {
        let provider = service
            .create_provider(ProviderInput {
                name: format!("{} fixture", kind.as_str()),
                kind,
                base_url: Url::parse(&format!("http://{address}/v1/")).unwrap(),
                settings: ProviderSettings::default(),
                enabled: true,
                secret: None,
            })
            .await
            .unwrap();
        let model = service
            .register_model(ModelInput {
                provider_id: provider.id,
                external_model_id: external_model_id.to_owned(),
                capabilities: ModelCapabilities {
                    structured_output: true,
                    ..ModelCapabilities::default()
                },
                settings: ModelSettings::default(),
                enabled: true,
            })
            .await
            .unwrap();
        let result = service
            .generate_structured(
                &work,
                model.id,
                &StructuredGenerationRequest {
                    model: external_model_id.to_owned(),
                    system: "Return an answer.".to_owned(),
                    user: "untrusted input".to_owned(),
                    schema_name: "answer".to_owned(),
                    schema: json!({
                        "type": "object",
                        "properties": {"answer": {"type": "string"}},
                        "required": ["answer"],
                        "additionalProperties": false
                    }),
                    missing_required_string_fallbacks: Vec::new(),
                    max_output_tokens: 8_192,
                    temperature: Some(0.0),
                    timeout: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(result.value["answer"], "ok");
    }

    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 6);
    assert_eq!(requests[0]["response_format"]["type"], "json_object");
    assert_eq!(requests[0]["max_tokens"], 32_768);
    assert_eq!(requests[0]["thinking"]["type"], "enabled");
    assert_eq!(requests[1]["max_completion_tokens"], 32_768);
    assert_eq!(requests[1]["thinking"]["type"], "enabled");
    assert_eq!(requests[2]["max_tokens"], 8_192);
    assert_eq!(requests[3]["response_format"]["type"], "json_schema");
    assert_eq!(requests[3]["max_completion_tokens"], 32_768);
    assert_eq!(requests[4]["response_format"]["type"], "json_schema");
    assert_eq!(requests[4]["max_tokens"], 32_768);
    assert_eq!(requests[5]["response_format"]["type"], "json_object");
    assert_eq!(requests[5]["max_completion_tokens"], 32_768);
    assert!(
        requests
            .iter()
            .all(|request| request.get("temperature").is_none())
    );

    server.abort();
}

#[tokio::test]
async fn provider_service_reuses_one_concurrency_gate_across_calls_and_clones() {
    let concurrency = Arc::new(ConcurrencyState::default());
    let app = Router::new()
        .route("/v1/models", get(concurrency_models))
        .with_state(concurrency.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let directory = tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let work = context(&state, "concurrency", directory.path().join("concurrency")).await;
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[6_u8; 32]).unwrap(),
    );
    let service = ProviderService::new(state, auth);
    service
        .set_provider_mode(&work, ProviderMode::LocalOnly, None)
        .await
        .unwrap();
    let provider = service
        .create_provider(ProviderInput {
            name: "bounded".to_owned(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: Url::parse(&format!("http://{address}/v1/")).unwrap(),
            settings: ProviderSettings {
                max_concurrency: 1,
                ..ProviderSettings::default()
            },
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();
    let first = service.clone();
    let second = service.clone();
    let third = service.clone();
    let fourth = service.clone();
    let _ = tokio::join!(
        first.test_provider(&work, provider.id),
        second.test_provider(&work, provider.id),
        third.test_provider(&work, provider.id),
        fourth.test_provider(&work, provider.id),
    );
    assert_eq!(concurrency.maximum.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn transport_enforces_retry_auth_timeout_and_redirect_contracts() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/retry", post(transient_response))
        .route("/unauthorized", post(unauthorized_response))
        .route("/invalid-content", post(invalid_content_response))
        .route("/invalid-json", post(invalid_json_response))
        .route("/redirect", post(redirect_response))
        .route("/slow", post(slow_response))
        .route("/delayed-body", post(delayed_body_response))
        .route("/interrupted-body", post(interrupted_body_response))
        .with_state(attempts.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let settings = ProviderSettings {
        max_retries: 1,
        timeout_ms: 250,
        connect_timeout_ms: 100,
        ..ProviderSettings::default()
    };
    let transport = ProviderTransport::new(settings.clone()).unwrap();
    let secret = SecretString::new("transport-secret");
    let endpoint = |path: &str| Url::parse(&format!("http://{address}{path}")).unwrap();
    let retry = transport
        .request_json(
            Method::POST,
            &endpoint("/retry"),
            ProviderMode::LocalOnly,
            &json!({}),
            RequestOptions::new(AuthStyle::Bearer, Some(&secret)),
        )
        .await
        .unwrap();
    assert_eq!(retry.body["ok"], true);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);

    let unauthorized = transport
        .request_json(
            Method::POST,
            &endpoint("/unauthorized"),
            ProviderMode::LocalOnly,
            &json!({}),
            RequestOptions::new(AuthStyle::Bearer, Some(&secret)),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        unauthorized,
        ProviderError::HttpStatus {
            status: 401,
            retryable: false
        }
    ));

    let invalid_content = transport
        .request_json(
            Method::POST,
            &endpoint("/invalid-content"),
            ProviderMode::LocalOnly,
            &json!({}),
            RequestOptions::new(AuthStyle::None, None)
                .with_timeout(Some(Duration::from_millis(250))),
        )
        .await
        .unwrap_err();
    assert_eq!(
        invalid_content.code(),
        "provider_response_content_type_invalid"
    );
    assert!(matches!(
        invalid_content,
        ProviderError::InvalidResponse("provider content type is not JSON")
    ));

    let invalid_json = transport
        .request_json(
            Method::POST,
            &endpoint("/invalid-json"),
            ProviderMode::LocalOnly,
            &json!({}),
            RequestOptions::new(AuthStyle::None, None),
        )
        .await
        .unwrap_err();
    assert_eq!(invalid_json.code(), "provider_response_json_invalid");

    let redirect = transport
        .request_json(
            Method::POST,
            &endpoint("/redirect"),
            ProviderMode::LocalOnly,
            &json!({}),
            RequestOptions::new(AuthStyle::None, None),
        )
        .await
        .unwrap_err();
    assert!(matches!(redirect, ProviderError::EndpointDenied));

    let timeout_transport = ProviderTransport::new(ProviderSettings {
        max_retries: 0,
        timeout_ms: 20,
        connect_timeout_ms: 10,
        ..ProviderSettings::default()
    })
    .unwrap();
    let timeout = timeout_transport
        .request_json(
            Method::POST,
            &endpoint("/slow"),
            ProviderMode::LocalOnly,
            &json!({}),
            RequestOptions::new(AuthStyle::None, None),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        timeout,
        ProviderError::Transport {
            code: "provider_timeout",
            retryable: true
        }
    ));

    let ambiguous_transport = ProviderTransport::new(ProviderSettings {
        max_retries: 2,
        timeout_ms: 20,
        connect_timeout_ms: 10,
        ..ProviderSettings::default()
    })
    .unwrap();
    let attempts_before_body_timeout = attempts.load(Ordering::SeqCst);
    let body_timeout = ambiguous_transport
        .request_json(
            Method::POST,
            &endpoint("/delayed-body"),
            ProviderMode::LocalOnly,
            &json!({}),
            RequestOptions::new(AuthStyle::None, None),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        body_timeout,
        ProviderError::Transport {
            code: "provider_response_timeout",
            retryable: false
        }
    ));
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        attempts_before_body_timeout + 1,
        "a response-body failure after HTTP success must not replay paid work"
    );

    let overridden = ambiguous_transport
        .request_json(
            Method::POST,
            &endpoint("/delayed-body"),
            ProviderMode::LocalOnly,
            &json!({}),
            RequestOptions::new(AuthStyle::None, None)
                .with_timeout(Some(Duration::from_millis(250))),
        )
        .await
        .unwrap();
    assert_eq!(overridden.body["ok"], true);

    let attempts_before_interrupted_body = attempts.load(Ordering::SeqCst);
    let interrupted_body = ambiguous_transport
        .request_json(
            Method::POST,
            &endpoint("/interrupted-body"),
            ProviderMode::LocalOnly,
            &json!({}),
            RequestOptions::new(AuthStyle::None, None)
                .with_timeout(Some(Duration::from_millis(250))),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            &interrupted_body,
            ProviderError::Transport {
                code: "provider_response_incomplete",
                retryable: false
            }
        ),
        "unexpected interrupted-body classification: {interrupted_body:?}"
    );
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        attempts_before_interrupted_body + 1,
        "an interrupted response body must not replay paid work"
    );

    server.abort();
}

#[tokio::test]
async fn provider_policy_rejects_remote_http_and_unsafe_endpoints() {
    let settings = ProviderSettings::default();
    let public_http = Url::parse("http://8.8.8.8/v1/").unwrap();
    assert!(
        validate_endpoint(&public_http, ProviderMode::RemoteAllowed, &settings)
            .await
            .is_err()
    );
    let metadata = Url::parse("http://169.254.169.254/v1/").unwrap();
    assert!(
        validate_endpoint(&metadata, ProviderMode::LocalOnly, &settings)
            .await
            .is_err()
    );
    assert_eq!(
        endpoint_url(&Url::parse("http://127.0.0.1/v1/").unwrap(), "embeddings")
            .unwrap()
            .path(),
        "/v1/embeddings"
    );
    assert_eq!(
        endpoint_url(
            &Url::parse("https://open.bigmodel.cn/api/paas/v4/").unwrap(),
            "chat/completions"
        )
        .unwrap()
        .path(),
        "/api/paas/v4/chat/completions"
    );
    assert_eq!(
        endpoint_url(
            &Url::parse("https://generativelanguage.googleapis.com/v1beta/openai/").unwrap(),
            "models"
        )
        .unwrap()
        .path(),
        "/v1beta/openai/models"
    );
    assert!(endpoint_url(&public_http, "../secret").is_err());
}

#[tokio::test]
async fn provider_disabled_mode_fails_before_remote_request() {
    let directory = tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let work = context(&state, "disabled", directory.path().join("disabled")).await;
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[5_u8; 32]).unwrap(),
    );
    let service = ProviderService::new(state, auth);
    let provider = service
        .create_provider(ProviderInput {
            name: "disabled".to_owned(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: Url::parse("https://api.example.test/v1/").unwrap(),
            settings: ProviderSettings::default(),
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();
    let model = service
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "model".to_owned(),
            capabilities: ModelCapabilities::default(),
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await
        .unwrap();
    let error = service
        .embed(
            &work,
            model.id,
            &EmbeddingRequest {
                model: "model".to_owned(),
                inputs: vec!["no network".to_owned()],
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderError::PrivacyDenied));
}

#[tokio::test]
async fn non_openai_adapter_rejects_an_openai_compatibility_override() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[8_u8; 32]).unwrap(),
    );
    let service = ProviderService::new(state, auth);
    let provider = service
        .create_provider(ProviderInput {
            name: "Anthropic fixture".to_owned(),
            kind: ProviderKind::AnthropicMessages,
            base_url: Url::parse("https://api.example.test/v1/").unwrap(),
            settings: ProviderSettings::default(),
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();

    let error = service
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "anthropic-model".to_owned(),
            capabilities: ModelCapabilities {
                structured_output: true,
                ..ModelCapabilities::default()
            },
            settings: ModelSettings {
                openai_structured_output_mode: OpenAiStructuredOutputMode::PromptOnly,
                ..ModelSettings::default()
            },
            enabled: true,
        })
        .await
        .unwrap_err();

    assert!(matches!(error, ProviderError::InvalidConfiguration(_)));
}
