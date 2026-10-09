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

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::State,
    http::{StatusCode, header},
    response::Response,
    routing::post,
};
use futures_util::stream;
use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_domain::{Revision, VaultContext, VaultId, VaultSlug};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderMode,
    ProviderService, ProviderSettings, StructuredGenerationRequest,
};
use mcp_vault_state::{StateStore, VaultStatus};
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::{net::TcpListener, time::sleep};
use url::Url;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scenario {
    Normal,
    SlowValid,
    IdleTimeout,
    TotalTimeout,
    EofWithoutFinish,
    InvalidSchema,
    MidStreamError,
    Truncated,
    WrongStopReason,
    AnthropicPing,
    ResponsesUnknown,
}

#[derive(Clone)]
struct FakeState {
    scenario: Arc<Mutex<Scenario>>,
    requests: Arc<AtomicUsize>,
    bodies: Arc<Mutex<Vec<Value>>>,
}

fn sse(value: &str) -> Bytes {
    Bytes::from(format!("data: {value}\n\n"))
}

fn delayed_sse_response(parts: Vec<(Duration, &'static str)>) -> Response {
    let body = Body::from_stream(stream::unfold(
        (parts, 0_usize),
        |(parts, index)| async move {
            let (delay, frame) = parts.get(index)?.to_owned();
            sleep(delay).await;
            Some((
                Result::<Bytes, Infallible>::Ok(Bytes::from_static(frame.as_bytes())),
                (parts, index + 1),
            ))
        },
    ));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(body)
        .unwrap()
}

fn chunks(scenario: Scenario) -> Vec<(Duration, Bytes)> {
    match scenario {
        Scenario::Normal => {
            let content = sse(r#"{"choices":[{"delta":{"content":"{\"answer\":\"ok\"}"}}]}"#);
            let split = content.len() / 2;
            let (left, right) = content.split_at(split);
            vec![
                (
                    Duration::from_millis(1),
                    sse(r#"{"choices":[{"delta":{"reasoning_content":"think "}}]}"#),
                ),
                (
                    Duration::from_millis(1),
                    sse(r#"{"choices":[{"delta":{"reasoning_content":"more"}}]}"#),
                ),
                (Duration::from_millis(1), Bytes::copy_from_slice(left)),
                (Duration::from_millis(1), Bytes::copy_from_slice(right)),
                (
                    Duration::from_millis(1),
                    sse(r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#),
                ),
                (Duration::from_millis(1), sse("[DONE]")),
            ]
        }
        Scenario::SlowValid => vec![
            (
                Duration::from_millis(15),
                sse(r#"{"choices":[{"delta":{"content":"{\"answer\":"}}]}"#),
            ),
            (
                Duration::from_millis(15),
                sse(r#"{"choices":[{"delta":{"reasoning_content":"long-think"}}]}"#),
            ),
            (
                Duration::from_millis(15),
                sse(r#"{"choices":[{"delta":{"content":"\"ok\"}"}}]}"#),
            ),
            (
                Duration::from_millis(1),
                sse(r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#),
            ),
            (Duration::from_millis(1), sse("[DONE]")),
        ],
        Scenario::IdleTimeout => vec![
            (
                Duration::from_millis(1),
                sse(r#"{"choices":[{"delta":{"reasoning_content":"start"}}]}"#),
            ),
            (
                Duration::from_millis(80),
                sse(r#"{"choices":[{"delta":{"content":"{\"answer\":\"late\"}"}}]}"#),
            ),
        ],
        Scenario::TotalTimeout => vec![
            (
                Duration::from_millis(15),
                sse(r#"{"choices":[{"delta":{"reasoning_content":"one"}}]}"#),
            ),
            (
                Duration::from_millis(15),
                sse(r#"{"choices":[{"delta":{"reasoning_content":"two"}}]}"#),
            ),
            (
                Duration::from_millis(15),
                sse(r#"{"choices":[{"delta":{"content":"{\"answer\":\"three\"}"}}]}"#),
            ),
        ],
        Scenario::EofWithoutFinish => vec![(
            Duration::from_millis(1),
            sse(r#"{"choices":[{"delta":{"content":"{\"answer\":\"ok\"}"}}]}"#),
        )],
        Scenario::InvalidSchema => vec![
            (
                Duration::from_millis(1),
                sse(r#"{"choices":[{"delta":{"content":"not-json"},"finish_reason":"stop"}]}"#),
            ),
            (Duration::from_millis(1), sse("[DONE]")),
        ],
        Scenario::MidStreamError => vec![(
            Duration::from_millis(1),
            sse(r#"{"error":{"type":"server_error","message":"upstream"}}"#),
        )],
        Scenario::Truncated => vec![(
            Duration::from_millis(1),
            Bytes::from_static(b"data: {\"choices\":[\n\n"),
        )],
        Scenario::WrongStopReason => vec![
            (
                Duration::from_millis(1),
                sse(r#"{"choices":[{"delta":{"content":"{\"answer\":\"ok\"}"}}]}"#),
            ),
            (
                Duration::from_millis(1),
                sse(r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#),
            ),
            (Duration::from_millis(1), sse("[DONE]")),
        ],
        Scenario::AnthropicPing | Scenario::ResponsesUnknown => Vec::new(),
    }
}

async fn fake_chat(State(state): State<FakeState>, Json(body): Json<Value>) -> Response {
    state.requests.fetch_add(1, Ordering::SeqCst);
    state.bodies.lock().unwrap().push(body);
    let scenario = *state.scenario.lock().unwrap();
    let parts = chunks(scenario);
    let body = Body::from_stream(stream::unfold(
        (parts, 0_usize),
        |(parts, index)| async move {
            let (delay, bytes) = parts.get(index)?.clone();
            sleep(delay).await;
            Some((Result::<Bytes, Infallible>::Ok(bytes), (parts, index + 1)))
        },
    ));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(body)
        .unwrap()
}

async fn fake_responses(State(state): State<FakeState>, Json(body): Json<Value>) -> Response {
    state.requests.fetch_add(1, Ordering::SeqCst);
    state.bodies.lock().unwrap().push(body);
    let scenario = *state.scenario.lock().unwrap();
    if scenario == Scenario::ResponsesUnknown {
        let mut parts = vec![(
            Duration::from_millis(1),
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"{\"}\n\n",
        )];
        parts.extend((0..8).map(|_| {
            (
                Duration::from_millis(5),
                "event: response.unknown\ndata: {\"type\":\"response.unknown\"}\n\n",
            )
        }));
        return delayed_sse_response(parts);
    }
    let frames = match scenario {
        Scenario::MidStreamError => {
            vec!["event: error\ndata: {\"type\":\"error\",\"message\":\"upstream\"}\n\n"]
        }
        Scenario::EofWithoutFinish => vec![
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\",\"status\":\"in_progress\"}}\n\n",
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"answer\\\":\\\"ok\\\"}\"}\n\n",
        ],
        _ => vec![
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\",\"status\":\"in_progress\"}}\n\n",
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"answer\\\":\\\"ok\\\"}\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"status\":\"completed\",\"output_text\":\"{\\\"answer\\\":\\\"ok\\\"}\",\"usage\":{\"input_tokens\":2,\"output_tokens\":3,\"total_tokens\":5}}}\n\n",
        ],
    };
    let body = Body::from_stream(stream::iter(
        frames
            .into_iter()
            .map(|frame| Ok::<Bytes, Infallible>(Bytes::from(frame))),
    ));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(body)
        .unwrap()
}

async fn fake_anthropic(State(state): State<FakeState>, Json(body): Json<Value>) -> Response {
    state.requests.fetch_add(1, Ordering::SeqCst);
    state.bodies.lock().unwrap().push(body);
    let scenario = *state.scenario.lock().unwrap();
    if scenario == Scenario::AnthropicPing {
        let mut parts = vec![(
            Duration::from_millis(1),
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
        )];
        parts.extend((0..8).map(|_| {
            (
                Duration::from_millis(5),
                "event: ping\ndata: {\"type\":\"ping\"}\n\n",
            )
        }));
        return delayed_sse_response(parts);
    }
    let frames = if scenario == Scenario::MidStreamError {
        vec![
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"upstream\"}}\n\n",
        ]
    } else if scenario == Scenario::EofWithoutFinish {
        vec![
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        ]
    } else {
        vec![
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"internal\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"{\\\"answer\\\":\\\"ok\\\"}\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ]
    };
    let body = Body::from_stream(stream::iter(
        frames
            .into_iter()
            .map(|frame| Ok::<Bytes, Infallible>(Bytes::from(frame))),
    ));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(body)
        .unwrap()
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

async fn provider_fixture(
    address: SocketAddr,
    state: &StateStore,
    context: &VaultContext,
    settings: ProviderSettings,
) -> (ProviderService, mcp_vault_domain::ModelId) {
    provider_fixture_kind(
        address,
        state,
        context,
        settings,
        ProviderKind::XiaomiMimo,
        "mimo-v2.5",
    )
    .await
}

async fn provider_fixture_kind(
    address: SocketAddr,
    state: &StateStore,
    context: &VaultContext,
    settings: ProviderSettings,
    kind: ProviderKind,
    external_model_id: &str,
) -> (ProviderService, mcp_vault_domain::ModelId) {
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[29; 32]).unwrap(),
    );
    let service = ProviderService::new(state.clone(), auth);
    service
        .set_provider_mode(context, ProviderMode::Enabled, None)
        .await
        .unwrap();
    let provider = service
        .create_provider(ProviderInput {
            name: "local streaming fixture".into(),
            kind,
            base_url: Url::parse(&format!("http://{address}/v1")).unwrap(),
            settings,
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();
    let model = service
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: external_model_id.into(),
            capabilities: ModelCapabilities {
                structured_output: true,
                ..Default::default()
            },
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await
        .unwrap();
    (service, model.id)
}

fn request_for_model(timeout: Option<Duration>, model: &str) -> StructuredGenerationRequest {
    StructuredGenerationRequest {
        model: model.into(),
        system: "Return one JSON object.".into(),
        user: "answer the fixture".into(),
        schema_name: "fixture_answer".into(),
        strict_function_schema: None,
        defer_local_schema_validation: false,
        strict_function_call: false,
        non_stream_json_object: false,
        schema: json!({"type":"object","additionalProperties":false,"required":["answer"],"properties":{"answer":{"type":"string"}}}),
        allow_additional_output_properties: false,
        missing_required_string_fallbacks: Vec::new(),
        max_output_tokens: 128,
        temperature: Some(0.0),
        timeout,
    }
}

fn request(timeout: Option<Duration>) -> StructuredGenerationRequest {
    request_for_model(timeout, "mimo-v2.5")
}

async fn server(state: FakeState) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route("/v1/chat/completions", post(fake_chat))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (address, task)
}

async fn server_with_routes(state: FakeState) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route("/v1/chat/completions", post(fake_chat))
        .route("/v1/responses", post(fake_responses))
        .route("/v1/messages", post(fake_anthropic))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (address, task)
}

#[tokio::test]
async fn mimo_streaming_separates_reasoning_and_content_and_validates_json() {
    let temp = tempdir().unwrap();
    let db = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context(&db, "stream-normal", temp.path().join("vault")).await;
    let state = FakeState {
        scenario: Arc::new(Mutex::new(Scenario::Normal)),
        requests: Arc::new(AtomicUsize::new(0)),
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let (address, server) = server(state.clone()).await;
    let (service, model) = provider_fixture(
        address,
        &db,
        &context,
        ProviderSettings {
            max_retries: 0,
            ..ProviderSettings::default()
        },
    )
    .await;
    let result = service
        .generate_structured(&context, model, &request(None))
        .await
        .unwrap();
    assert_eq!(result.value["answer"], "ok");
    assert!(
        result.usage.is_none(),
        "missing stream usage must remain unknown"
    );
    assert_eq!(state.requests.load(Ordering::SeqCst), 1);
    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["stream"], true);
    drop(bodies);
    server.abort();
}

#[tokio::test]
async fn mimo_stream_can_exceed_legacy_timeout_without_exceeding_stream_total() {
    let temp = tempdir().unwrap();
    let db = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context(&db, "stream-slow", temp.path().join("vault")).await;
    let state = FakeState {
        scenario: Arc::new(Mutex::new(Scenario::SlowValid)),
        requests: Arc::new(AtomicUsize::new(0)),
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let (address, server) = server(state.clone()).await;
    let settings = ProviderSettings {
        timeout_ms: 25,
        connect_timeout_ms: 5,
        stream_first_event_timeout_ms: 40,
        stream_idle_timeout_ms: 40,
        stream_total_timeout_ms: 200,
        max_retries: 0,
        ..ProviderSettings::default()
    };
    let (service, model) = provider_fixture(address, &db, &context, settings).await;
    let result = service
        .generate_structured(&context, model, &request(None))
        .await
        .unwrap();
    assert_eq!(result.value["answer"], "ok");
    assert_eq!(state.requests.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn mimo_stream_failures_are_bounded_and_do_not_retry() {
    let cases = [
        (
            Scenario::IdleTimeout,
            ProviderSettings {
                stream_first_event_timeout_ms: 40,
                stream_idle_timeout_ms: 20,
                stream_total_timeout_ms: 200,
                max_retries: 0,
                ..ProviderSettings::default()
            },
            None,
        ),
        (
            Scenario::TotalTimeout,
            ProviderSettings {
                stream_first_event_timeout_ms: 20,
                stream_idle_timeout_ms: 20,
                stream_total_timeout_ms: 25,
                max_retries: 0,
                ..ProviderSettings::default()
            },
            None,
        ),
        (
            Scenario::TotalTimeout,
            ProviderSettings {
                stream_first_event_timeout_ms: 40,
                stream_idle_timeout_ms: 100,
                stream_total_timeout_ms: 200,
                max_retries: 0,
                ..ProviderSettings::default()
            },
            Some(Duration::from_millis(25)),
        ),
        (
            Scenario::EofWithoutFinish,
            ProviderSettings {
                max_retries: 0,
                ..ProviderSettings::default()
            },
            None,
        ),
        (
            Scenario::InvalidSchema,
            ProviderSettings {
                max_retries: 0,
                ..ProviderSettings::default()
            },
            None,
        ),
        (
            Scenario::MidStreamError,
            ProviderSettings {
                max_retries: 0,
                ..ProviderSettings::default()
            },
            None,
        ),
        (
            Scenario::Truncated,
            ProviderSettings {
                max_retries: 0,
                ..ProviderSettings::default()
            },
            None,
        ),
        (
            Scenario::WrongStopReason,
            ProviderSettings {
                max_retries: 0,
                ..ProviderSettings::default()
            },
            None,
        ),
    ];
    for (index, (scenario, settings, timeout)) in cases.into_iter().enumerate() {
        let temp = tempdir().unwrap();
        let db = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = context(
            &db,
            &format!("stream-failure-{index}"),
            temp.path().join("vault"),
        )
        .await;
        let state = FakeState {
            scenario: Arc::new(Mutex::new(scenario)),
            requests: Arc::new(AtomicUsize::new(0)),
            bodies: Arc::new(Mutex::new(Vec::new())),
        };
        let (address, server) = server(state.clone()).await;
        let (service, model) = provider_fixture(address, &db, &context, settings).await;
        assert!(
            service
                .generate_structured(&context, model, &request(timeout))
                .await
                .is_err(),
            "scenario {scenario:?} unexpectedly succeeded"
        );
        assert_eq!(
            state.requests.load(Ordering::SeqCst),
            1,
            "scenario {scenario:?} retried"
        );
        server.abort();
    }
}

#[tokio::test]
async fn all_openai_chat_presets_use_streaming_chat_wire_and_expected_dialect() {
    let temp = tempdir().unwrap();
    let db = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context(&db, "stream-presets", temp.path().join("vault")).await;
    let state = FakeState {
        scenario: Arc::new(Mutex::new(Scenario::Normal)),
        requests: Arc::new(AtomicUsize::new(0)),
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let (address, server) = server(state.clone()).await;
    let cases = [
        (ProviderKind::OpenAiCompatible, "generic", "max_tokens"),
        (ProviderKind::DeepSeek, "deepseek-chat", "max_tokens"),
        (
            ProviderKind::XiaomiMimo,
            "mimo-v2.5",
            "max_completion_tokens",
        ),
        (ProviderKind::ZhipuGlm, "glm-5.2", "max_tokens"),
        (
            ProviderKind::MoonshotKimi,
            "kimi-k2.6",
            "max_completion_tokens",
        ),
        (ProviderKind::GoogleGemini, "gemini-3.8-flash", "max_tokens"),
        (
            ProviderKind::AlibabaQwen,
            "qwen3.8-max",
            "max_completion_tokens",
        ),
    ];
    for (kind, model_name, token_field) in cases {
        let (service, model) = provider_fixture_kind(
            address,
            &db,
            &context,
            ProviderSettings {
                max_retries: 0,
                ..ProviderSettings::default()
            },
            kind,
            model_name,
        )
        .await;
        let result = service
            .generate_structured(&context, model, &request_for_model(None, model_name))
            .await
            .unwrap();
        assert_eq!(result.value["answer"], "ok");
        let body = state.bodies.lock().unwrap().last().cloned().unwrap();
        assert_eq!(body["stream"], true, "{kind:?} did not request streaming");
        assert!(
            body.get(token_field).is_some(),
            "{kind:?} missing {token_field}"
        );
    }
    assert_eq!(state.requests.load(Ordering::SeqCst), 7);
    server.abort();
}

#[tokio::test]
async fn responses_stream_uses_semantic_events_without_done_sentinel() {
    let temp = tempdir().unwrap();
    let db = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context(&db, "stream-responses", temp.path().join("vault")).await;
    let state = FakeState {
        scenario: Arc::new(Mutex::new(Scenario::Normal)),
        requests: Arc::new(AtomicUsize::new(0)),
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let (address, server) = server_with_routes(state.clone()).await;
    let (service, model) = provider_fixture_kind(
        address,
        &db,
        &context,
        ProviderSettings {
            max_retries: 0,
            ..ProviderSettings::default()
        },
        ProviderKind::OpenAiResponses,
        "gpt-test",
    )
    .await;
    let result = service
        .generate_structured(&context, model, &request_for_model(None, "gpt-test"))
        .await
        .unwrap();
    assert_eq!(result.value["answer"], "ok");
    assert_eq!(result.usage.unwrap()["total_tokens"], 5);
    assert_eq!(state.bodies.lock().unwrap()[0]["stream"], true);
    server.abort();
}

#[tokio::test]
async fn anthropic_stream_preserves_block_indexes_and_terminal_usage() {
    let temp = tempdir().unwrap();
    let db = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context(&db, "stream-anthropic", temp.path().join("vault")).await;
    let state = FakeState {
        scenario: Arc::new(Mutex::new(Scenario::Normal)),
        requests: Arc::new(AtomicUsize::new(0)),
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let (address, server) = server_with_routes(state.clone()).await;
    let (service, model) = provider_fixture_kind(
        address,
        &db,
        &context,
        ProviderSettings {
            max_retries: 0,
            ..ProviderSettings::default()
        },
        ProviderKind::AnthropicMessages,
        "claude-test",
    )
    .await;
    let result = service
        .generate_structured(&context, model, &request_for_model(None, "claude-test"))
        .await
        .unwrap();
    assert_eq!(result.value["answer"], "ok");
    assert_eq!(result.usage.unwrap()["output_tokens"], 3);
    assert_eq!(state.bodies.lock().unwrap()[0]["stream"], true);
    server.abort();
}

#[tokio::test]
async fn native_stream_errors_and_missing_terminal_fail_closed() {
    for scenario in [Scenario::MidStreamError, Scenario::EofWithoutFinish] {
        let temp = tempdir().unwrap();
        let db = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = context(&db, "native-failure", temp.path().join("vault")).await;
        let state = FakeState {
            scenario: Arc::new(Mutex::new(scenario)),
            requests: Arc::new(AtomicUsize::new(0)),
            bodies: Arc::new(Mutex::new(Vec::new())),
        };
        let (address, server) = server_with_routes(state.clone()).await;
        let (responses, response_model) = provider_fixture_kind(
            address,
            &db,
            &context,
            ProviderSettings {
                max_retries: 0,
                ..ProviderSettings::default()
            },
            ProviderKind::OpenAiResponses,
            "gpt-test",
        )
        .await;
        assert!(
            responses
                .generate_structured(
                    &context,
                    response_model,
                    &request_for_model(None, "gpt-test")
                )
                .await
                .is_err()
        );
        let (anthropic, anthropic_model) = provider_fixture_kind(
            address,
            &db,
            &context,
            ProviderSettings {
                max_retries: 0,
                ..ProviderSettings::default()
            },
            ProviderKind::AnthropicMessages,
            "claude-test",
        )
        .await;
        assert!(
            anthropic
                .generate_structured(
                    &context,
                    anthropic_model,
                    &request_for_model(None, "claude-test")
                )
                .await
                .is_err()
        );
        assert_eq!(state.requests.load(Ordering::SeqCst), 2);
        server.abort();
    }
}

#[tokio::test]
async fn native_ignored_events_do_not_extend_idle_deadline() {
    let cases = [
        (
            Scenario::ResponsesUnknown,
            ProviderKind::OpenAiResponses,
            "gpt-test",
        ),
        (
            Scenario::AnthropicPing,
            ProviderKind::AnthropicMessages,
            "claude-test",
        ),
    ];
    for (scenario, kind, model_name) in cases {
        let temp = tempdir().unwrap();
        let db = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let context = context(&db, "native-idle", temp.path().join("vault")).await;
        let state = FakeState {
            scenario: Arc::new(Mutex::new(scenario)),
            requests: Arc::new(AtomicUsize::new(0)),
            bodies: Arc::new(Mutex::new(Vec::new())),
        };
        let (address, server) = server_with_routes(state.clone()).await;
        let settings = ProviderSettings {
            stream_first_event_timeout_ms: 40,
            stream_idle_timeout_ms: 20,
            stream_total_timeout_ms: 200,
            max_retries: 0,
            ..ProviderSettings::default()
        };
        let (service, model) =
            provider_fixture_kind(address, &db, &context, settings, kind, model_name).await;
        let error = service
            .generate_structured(&context, model, &request_for_model(None, model_name))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "provider_stream_idle_timeout");
        assert_eq!(state.requests.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
