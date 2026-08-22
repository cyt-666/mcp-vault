use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use mcp_vault_auth::{AuthService, MasterKeyRing, SecretString};
use mcp_vault_domain::{Revision, VaultContext, VaultId, VaultSlug};
use mcp_vault_providers::{
    AuthStyle, EmbeddingInput, EmbeddingRequest, EmbeddingSourceRef, EmbeddingSourceResolver,
    ModelCapabilities, ModelInput, ProviderError, ProviderInput, ProviderKind, ProviderMode,
    ProviderService, ProviderSettings, ProviderTransport, StructuredGenerationRequest,
    endpoint_url, validate_endpoint,
};
use mcp_vault_state::{StateStore, VaultStatus};
use reqwest::Method;
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::net::TcpListener;
use url::Url;

async fn fake_chat() -> Json<serde_json::Value> {
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

async fn redirect_response() -> impl IntoResponse {
    (StatusCode::FOUND, [(header::LOCATION, "/retry")], "")
}

async fn slow_response() -> impl IntoResponse {
    tokio::time::sleep(Duration::from_millis(100)).await;
    Json(json!({"ok": true}))
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
                max_output_tokens: 32,
                temperature: Some(0.0),
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
                max_output_tokens: 32,
                temperature: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(invalid_schema, ProviderError::SchemaValidation));

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
    let hits = embeddings
        .search(&work, embedding.id, &[1.0, 0.0, 0.0], 10)
        .await
        .unwrap();
    assert_eq!(hits[0].embedding.object_id, "note-a");
    assert!(
        embeddings
            .search(&other, embedding.id, &[1.0, 0.0, 0.0], 10)
            .await
            .unwrap()
            .is_empty()
    );
    let coverage = embeddings.coverage(&work, embedding.id).await.unwrap();
    assert_eq!(coverage.total, 2);
    assert_eq!(coverage.objects, 2);
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
            settings: json!({}),
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
            settings: json!({}),
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
    assert!(job.payload.to_string().contains("note-a"));
    assert!(!job.payload.to_string().contains("first"));

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
        .route("/redirect", post(redirect_response))
        .route("/slow", post(slow_response))
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
            Some(&secret),
            AuthStyle::Bearer,
            &json!({}),
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
            Some(&secret),
            AuthStyle::Bearer,
            &json!({}),
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
            None,
            AuthStyle::None,
            &json!({}),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        invalid_content,
        ProviderError::InvalidResponse("provider content type is not JSON")
    ));

    let redirect = transport
        .request_json(
            Method::POST,
            &endpoint("/redirect"),
            ProviderMode::LocalOnly,
            None,
            AuthStyle::None,
            &json!({}),
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
            None,
            AuthStyle::None,
            &json!({}),
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
            settings: json!({}),
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
