use std::{
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::post};
use mcp_vault_auth::{AuthService, MasterKeyRing, SecretString};
use mcp_vault_core::VaultCore;
use mcp_vault_domain::{
    Actor, EvidenceRefId, ExtractionSetId, Revision, SourcePlane, VaultContext, VaultId, VaultPath,
    VaultPathPolicy, VaultSlug,
};
use mcp_vault_eval::{
    ArmSemanticRuntime, ComparisonArm, EvalSource, LiveProviderRequest, LiveTransportRequestBudget,
    ProviderAppBoundary, ProviderServiceAppBoundary, SemanticMemoryAppBoundary,
    SemanticMemoryServiceAppBoundary, semantic_a80_provider_templates,
    semantic_live_provider_templates,
};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderMode,
    ProviderService, ProviderSettings, StructuredJsonFinishReason, StructuredJsonParseIssue,
    StructuredJsonParserCategory,
};
use mcp_vault_state::{StateStore, VaultStatus};
use mcp_vault_storage_fs::{DurabilityPolicy, StorageOptions};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tracing::instrument::WithSubscriber;
use url::Url;

#[derive(Clone)]
struct FakeState {
    calls: Arc<AtomicUsize>,
    expected_namespace: Arc<std::sync::Mutex<Option<String>>>,
    empty: bool,
    fail_second: bool,
    invalid_schema: bool,
    invalid_assertion_status: bool,
    extra_observation_property: bool,
    body_context_overlap: bool,
    all_m6_stages: bool,
    expected_json_object: bool,
    strict_reply: StrictReply,
}

#[derive(Clone, Copy)]
enum StrictReply {
    Valid,
    NoTool,
    MultipleTools,
    WrongName,
    InvalidArguments,
    MixedContent,
    TooLarge,
    WrongFinish,
    MissingId,
}

const PRIVATE_JSON_MARKER: &str = "PRIVATE_M6_MARKER-61d3";
const MALFORMED_STRUCTURED_JSON: &str = r#"{"private_note":"PRIVATE_M6_MARKER-61d3",}"#;

#[derive(Clone)]
struct MalformedJsonState {
    calls: Arc<AtomicUsize>,
}

async fn malformed_json_chat(
    State(state): State<MalformedJsonState>,
    Json(request): Json<Value>,
) -> axum::response::Response {
    state.calls.fetch_add(1, Ordering::SeqCst);
    assert_eq!(request["stream"], json!(true));
    let content = json!({
        "id": "malformed-json-fixture",
        "model": "externalmodel",
        "choices": [{"index": 0, "delta": {"content": MALFORMED_STRUCTURED_JSON}}]
    });
    let stop = json!({
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    });
    axum::response::Response::builder()
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from(format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            content, stop
        )))
        .unwrap()
        .into_response()
}

#[derive(Clone)]
struct CapturedLogWriter(Arc<std::sync::Mutex<Vec<u8>>>);

impl io::Write for CapturedLogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

async fn fake_chat(
    State(state): State<FakeState>,
    Json(request): Json<Value>,
) -> axum::response::Response {
    let call = state.calls.fetch_add(1, Ordering::SeqCst);
    let user_input = request["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_str())
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .unwrap_or_else(
            || json!({"evidence_namespace":"fixture-namespace","blocks":[{"evidence_index":1}]}),
        );
    let namespace = user_input["evidence_namespace"]
        .as_str()
        .unwrap_or("fixture-namespace");
    let invalid_index = user_input["blocks"]
        .as_array()
        .map_or(1, |blocks| blocks.len() + 1);
    let strict_function_call = request["tools"].is_array();
    let strict_tool_name = request["tools"][0]["function"]["name"]
        .as_str()
        .unwrap_or("semantic_memory_observation");
    if state.expected_json_object && !strict_function_call {
        assert_eq!(request["response_format"]["type"], "json_object");
        assert_eq!(request["thinking"]["type"], "enabled");
    }
    let schema_from_system;
    let schema = if strict_function_call {
        assert_eq!(request["tools"].as_array().unwrap().len(), 1);
        assert_eq!(request["tools"][0]["type"], "function");
        assert_eq!(request["tools"][0]["function"]["strict"], true);
        assert_eq!(request["stream"], false);
        assert!(matches!(
            strict_tool_name,
            "semantic_memory_observation"
                | "semantic_memory_composition"
                | "semantic_memory_m2_organization"
                | "semantic_memory_task_answer"
        ));
        assert_eq!(request["tool_choice"], "auto");
        assert_eq!(request["thinking"]["type"], "disabled");
        assert!(request.get("response_format").is_none());
        &request["tools"][0]["function"]["parameters"]
    } else if request["response_format"]["type"] == "json_object" {
        let system = request["messages"][0]["content"].as_str().unwrap();
        if call == 0 {
            assert!(system.contains("只允许当前 JSON Schema 声明的字段"));
            assert!(system.contains("不得增补解释、来源摘录、调试信息"));
            assert!(system.contains("顶层 evidence_namespace"));
            assert!(system.contains("outcome 必填"));
        }
        let encoded_schema = system.split_once("exactly:\n").unwrap().1;
        schema_from_system = serde_json::from_str::<Value>(encoded_schema).unwrap();
        &schema_from_system
    } else {
        &request["response_format"]["json_schema"]["schema"]
    };
    let required = schema["required"].as_array().unwrap();
    if state.all_m6_stages {
        match strict_tool_name {
            "semantic_memory_observation" => {
                assert_eq!(required.len(), 3);
                assert_eq!(
                    schema["properties"]["observations"]["items"]["required"]
                        .as_array()
                        .unwrap()
                        .len(),
                    14
                );
            }
            "semantic_memory_composition" => {
                assert_eq!(required, &[json!("cards")]);
                assert_eq!(
                    schema["properties"]["cards"]["items"]["required"],
                    json!(["title", "observation_indices"])
                );
            }
            "semantic_memory_m2_organization" => {
                let item = &schema["properties"]["actions"]["items"];
                assert_eq!(item["required"].as_array().unwrap().len(), 8);
                assert_eq!(item["additionalProperties"], false);
                assert!(item["properties"]["item_kind"].get("enum").is_none());
                assert!(item["properties"]["support_operator"].get("enum").is_none());
            }
            "semantic_memory_task_answer" => {
                assert_eq!(
                    schema["required"],
                    json!(["answer", "answerability", "status", "evidence_ids"])
                );
            }
            _ => unreachable!(),
        }
    } else if call == 0 {
        if strict_function_call {
            assert_eq!(
                required,
                &[
                    json!("evidence_namespace"),
                    json!("outcome"),
                    json!("observations")
                ]
            );
            let observation = &schema["properties"]["observations"]["items"];
            assert_eq!(observation["required"].as_array().unwrap().len(), 14);
            assert_eq!(observation["additionalProperties"], false);
            assert_eq!(observation["properties"]["result"]["type"], "string");
            assert_eq!(observation["properties"]["uncertainty"]["type"], "string");
        } else {
            assert_eq!(
                required,
                &[
                    json!("evidence_namespace"),
                    json!("outcome"),
                    json!("observations")
                ]
            );
        }
        assert_eq!(schema["properties"]["outcome"]["type"], "string");
        assert_eq!(
            schema["properties"]["outcome"]["enum"],
            json!(["success_nonempty", "success_empty"])
        );
        assert!(schema["properties"].get("cards").is_none());
        let expected = state.expected_namespace.lock().unwrap().clone();
        if let Some(expected) = expected {
            assert_eq!(
                schema["properties"]["evidence_namespace"]["enum"],
                json!([expected])
            );
            let block_count = user_input["blocks"].as_array().unwrap().len();
            let expected_indices = (1..=block_count)
                .map(|index| json!(index))
                .collect::<Vec<_>>();
            for key in ["body_block_indices", "context_block_indices"] {
                assert_eq!(
                    schema["properties"]["observations"]["items"]["properties"][key]["items"]["enum"],
                    json!(expected_indices)
                );
            }
            assert_eq!(
                schema["properties"]["observations"]["items"]["properties"]["source_time_scope"]["properties"]
                    ["evidence_block_indices"]["items"]["enum"],
                json!(expected_indices)
            );
        }
    } else {
        assert_eq!(required, &[json!("cards")]);
        let card = &schema["properties"]["cards"]["items"];
        assert_eq!(card["required"], json!(["title", "observation_indices"]));
        assert!(card["properties"].get("kind").is_none());
        assert!(card["properties"].get("scope").is_none());
        assert!(card["properties"].get("assertion_status").is_none());
    }
    if state.fail_second && call == 1 {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let status = if state.invalid_assertion_status {
        "invalid_status_token"
    } else {
        "source_asserted"
    };
    let body_index = if state.invalid_schema {
        invalid_index
    } else {
        1
    };
    let mut first_observation = json!({
        "kind":"preference","statement":"Use reviewed changes.","scope":"project",
        "assertion_status":status,"admission_reason":"test","value_for_future_work":"preserve review",
        "body_block_indices":[body_index],
        "source_time_scope":{"status":"unknown","value":"","evidence_block_indices":[]},
        "conditions":[],"exceptions":[],"ordered_steps":[],"result":"","uncertainty":"","context_block_indices":[]
    });
    if state.body_context_overlap && call == 0 {
        first_observation["context_block_indices"] = json!([body_index]);
    }
    if state.extra_observation_property {
        first_observation["debug_note"] = json!("unexpected");
    }
    let second_observation = json!({
        "kind":"constraint","statement":"Keep the boundary isolated.","scope":"project",
        "assertion_status":"source_asserted","admission_reason":"test","value_for_future_work":"preserve isolation",
        "body_block_indices":[1],
        "source_time_scope":{"status":"unknown","value":"","evidence_block_indices":[]},
        "conditions":[],"exceptions":[],"ordered_steps":[],"result":"","uncertainty":"","context_block_indices":[]
    });
    let content = if state.all_m6_stages {
        match strict_tool_name {
            "semantic_memory_observation" => json!({
                "evidence_namespace":namespace,
                "outcome":"success_nonempty",
                "observations":[first_observation]
            }),
            "semantic_memory_composition" => {
                json!({"cards":[{"title":"Strict stage matrix","observation_indices":[0]}]})
            }
            "semantic_memory_m2_organization" => json!({"actions":[{
                "action":"no_change","candidate_ids":["candidate-1"],"card_ref":"",
                "title":"","content":"","item_kind":"","support_operator":"","reason":""
            }]}),
            "semantic_memory_task_answer" => json!({
                "answer":"Supported fixture answer.","answerability":"answerable",
                "status":"supported","evidence_ids":["evidence-1"]
            }),
            _ => unreachable!(),
        }
    } else if call > 0 {
        if state.body_context_overlap {
            json!({"cards":[{"title":"Overlap normalized","observation_indices":[0]}]})
        } else {
            json!({"cards":[{"title":"Reviewed changes","observation_indices":[0]},{"title":"Isolated boundary","observation_indices":[1]}]})
        }
    } else {
        let observations = if state.empty {
            json!([])
        } else if strict_function_call {
            json!([first_observation])
        } else {
            json!([first_observation, second_observation])
        };
        let outcome = if state.empty {
            "success_empty"
        } else {
            "success_nonempty"
        };
        json!({"evidence_namespace":namespace,"outcome":outcome,"observations":observations})
    };
    if strict_function_call {
        let mut arguments = content.to_string();
        let mut function_name = strict_tool_name;
        let mut message_content = Value::Null;
        let mut finish_reason = "tool_calls";
        let mut include_id = true;
        let tool_count = match state.strict_reply {
            StrictReply::Valid
            | StrictReply::NoTool
            | StrictReply::WrongName
            | StrictReply::InvalidArguments
            | StrictReply::MixedContent
            | StrictReply::TooLarge
            | StrictReply::WrongFinish
            | StrictReply::MissingId => 1,
            StrictReply::MultipleTools => 2,
        };
        match state.strict_reply {
            StrictReply::Valid | StrictReply::MultipleTools | StrictReply::MissingId => {}
            StrictReply::NoTool => {
                finish_reason = "stop";
            }
            StrictReply::WrongName => function_name = "wrong_function",
            StrictReply::InvalidArguments => arguments = "not-json".to_owned(),
            StrictReply::MixedContent => message_content = json!("unexpected assistant content"),
            StrictReply::TooLarge => arguments = "x".repeat(1024 * 1024 + 1),
            StrictReply::WrongFinish => finish_reason = "stop",
        }
        if matches!(state.strict_reply, StrictReply::MissingId) {
            include_id = false;
        }
        let mut tool_calls = Vec::new();
        if !matches!(state.strict_reply, StrictReply::NoTool) {
            for _ in 0..tool_count {
                let mut tool_call = json!({
                    "index":0,
                    "type":"function",
                    "function":{"name":function_name,"arguments":arguments}
                });
                if include_id {
                    tool_call["id"] = json!("call-1");
                }
                tool_calls.push(tool_call);
            }
        }
        let reply = json!({
            "id":"fixture-tool","model":"externalmodel",
            "choices":[{"index":0,"message":{"role":"assistant","content":message_content,"tool_calls":tool_calls},"finish_reason":finish_reason}],
            "usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}
        });
        return axum::Json(reply).into_response();
    }
    let payload = content.to_string();
    let first = json!({"id":"fixture","model":"externalmodel","choices":[{
        "index":0,"delta":{"content":payload}
    }]});
    let finish = json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
    axum::response::Response::builder()
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from(format!(
            "data: {first}\n\ndata: {finish}\n\ndata: [DONE]\n\n"
        )))
        .unwrap()
        .into_response()
}

#[derive(Clone)]
struct A80FakeState {
    calls: Arc<AtomicUsize>,
}

async fn a80_fake_chat(
    State(state): State<A80FakeState>,
    Json(request): Json<Value>,
) -> axum::response::Response {
    state.calls.fetch_add(1, Ordering::SeqCst);
    assert_eq!(request["response_format"]["type"], "json_object");
    let messages = request["messages"].as_array().unwrap();
    let input: Value =
        serde_json::from_str(messages.last().unwrap()["content"].as_str().unwrap()).unwrap();
    let blocks = input["blocks"].as_array().unwrap();
    assert!(!blocks.is_empty() && blocks.len() <= 80);
    let selected = blocks.get(1).unwrap_or(&blocks[0]);
    let output = json!({
        "claims":[{
            "statement":selected["text"],
            "evidence_indices":[selected["evidence_index"]],
            "kind":"decision"
        }]
    });
    let chunk = json!({
        "id":"a80-fixture",
        "model":"externalmodel",
        "choices":[{"index":0,"delta":{"content":output.to_string()}}]
    });
    let stop = json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
    axum::response::Response::builder()
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from(format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            chunk, stop
        )))
        .unwrap()
        .into_response()
}

struct Fixture {
    state: StateStore,
    context: VaultContext,
    core: VaultCore,
    source_root: PathBuf,
    db: PathBuf,
    history: PathBuf,
}

async fn fixture(root: &Path, name: &str) -> Fixture {
    let source_root = root.join(name).join("vault");
    let db = root.join(name).join("state.sqlite3");
    let history = root.join(name).join("history");
    let state = StateStore::connect_and_migrate(&format!("sqlite://{}", db.display()))
        .await
        .unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new(name).unwrap(),
        source_root.clone(),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&context, name, VaultStatus::Active)
        .await
        .unwrap();
    state
        .settings()
        .set_vault(
            &context,
            "memory.units.policy",
            &json!({"enabled":true,"request_timeout_seconds":300}),
            mcp_vault_domain::WritePrecondition::Unconditional,
            None,
        )
        .await
        .unwrap();
    let core = VaultCore::new(
        state.clone(),
        history.clone(),
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
        Default::default(),
    );
    Fixture {
        state,
        context,
        core,
        source_root,
        db,
        history,
    }
}

fn runtime(f: &Fixture) -> ArmSemanticRuntime {
    ArmSemanticRuntime {
        state: f.state.clone(),
        context: f.context.clone(),
        core: f.core.clone(),
        source_root: f.source_root.clone(),
        state_db_path: f.db.clone(),
        history_root: f.history.clone(),
    }
}

async fn create_eval_source(
    fixture: &Fixture,
    logical_id: &str,
    path: &str,
    bytes: &[u8],
) -> EvalSource {
    let path = VaultPath::parse(path).unwrap();
    let created = fixture
        .core
        .create_bytes(
            &fixture.context,
            &path,
            bytes,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    EvalSource {
        logical_id: logical_id.to_owned(),
        synthetic_placeholder: false,
        vault_id: fixture.context.id().to_string(),
        file_id: created.file.id.to_string(),
        path: path.as_str().to_owned(),
        file_revision: created.file.current_revision.value(),
        content_hash: format!("{:x}", Sha256::digest(bytes)),
        logical_block_count: 0,
        source_revision_id: "pending".into(),
        authorization_revision: 1,
        profile_id: "semantic-profile-v1".into(),
        rules_revision: 1,
        source_generation: 1,
        extraction_commit_sequence: 1,
    }
}

async fn latest_extraction(fixture: &Fixture) -> (ExtractionSetId, String) {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", fixture.db.display()))
        .await
        .unwrap();
    let (id, state): (String, String) = sqlx::query_as(
        "SELECT extraction_set_id,state FROM semantic_extraction_sets WHERE vault_id=? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(fixture.context.id().to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    pool.close().await;
    (ExtractionSetId::parse(&id).unwrap(), state)
}

async fn running_extraction_count(fixture: &Fixture) -> i64 {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", fixture.db.display()))
        .await
        .unwrap();
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM semantic_extraction_sets WHERE vault_id=? AND state='running'",
    )
    .bind(fixture.context.id().to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    pool.close().await;
    count
}

#[derive(Clone, Copy)]
struct ProviderFixtureSettings {
    empty: bool,
    fail_second: bool,
    invalid_schema: bool,
    invalid_assertion_status: bool,
    provider_kind: ProviderKind,
    extra_observation_property: bool,
    body_context_overlap: bool,
    all_m6_stages: bool,
    prompt_override: Option<&'static str>,
    strict_reply: StrictReply,
}

async fn provider_fixture(
    fixture: &Fixture,
    empty: bool,
    fail_second: bool,
    invalid_schema: bool,
    invalid_assertion_status: bool,
    provider_kind: ProviderKind,
    extra_observation_property: bool,
) -> (
    ProviderServiceAppBoundary,
    Arc<AtomicUsize>,
    Arc<std::sync::Mutex<Option<String>>>,
    tokio::task::JoinHandle<()>,
    Vec<mcp_vault_eval::ProviderStageTemplate>,
) {
    provider_fixture_with_settings(
        fixture,
        ProviderFixtureSettings {
            empty,
            fail_second,
            invalid_schema,
            invalid_assertion_status,
            provider_kind,
            extra_observation_property,
            body_context_overlap: false,
            all_m6_stages: false,
            prompt_override: None,
            strict_reply: StrictReply::Valid,
        },
    )
    .await
}

async fn provider_fixture_with_settings(
    fixture: &Fixture,
    fixture_settings: ProviderFixtureSettings,
) -> (
    ProviderServiceAppBoundary,
    Arc<AtomicUsize>,
    Arc<std::sync::Mutex<Option<String>>>,
    tokio::task::JoinHandle<()>,
    Vec<mcp_vault_eval::ProviderStageTemplate>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let expected_namespace = Arc::new(std::sync::Mutex::new(None));
    let app = Router::new()
        .route("/v1/chat/completions", post(fake_chat))
        .with_state(FakeState {
            calls: calls.clone(),
            expected_namespace: expected_namespace.clone(),
            empty: fixture_settings.empty,
            fail_second: fixture_settings.fail_second,
            invalid_schema: fixture_settings.invalid_schema,
            invalid_assertion_status: fixture_settings.invalid_assertion_status,
            extra_observation_property: fixture_settings.extra_observation_property,
            body_context_overlap: fixture_settings.body_context_overlap,
            all_m6_stages: fixture_settings.all_m6_stages,
            expected_json_object: fixture_settings.provider_kind == ProviderKind::XiaomiMimo,
            strict_reply: fixture_settings.strict_reply,
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let auth = AuthService::new(
        fixture.state.auth(),
        MasterKeyRing::from_bytes(1, &[9; 32]).unwrap(),
    );
    let service = ProviderService::new(fixture.state.clone(), auth);
    service
        .set_provider_mode(&fixture.context, ProviderMode::LocalOnly, None)
        .await
        .unwrap();
    let settings = ProviderSettings {
        max_retries: 0,
        ..ProviderSettings::default()
    };
    let provider = service
        .create_provider(ProviderInput {
            name: "fixture".into(),
            kind: fixture_settings.provider_kind,
            base_url: Url::parse(&format!("http://{addr}/v1")).unwrap(),
            settings,
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();
    let model = service
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "externalmodel".into(),
            capabilities: ModelCapabilities {
                structured_output: true,
                ..Default::default()
            },
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await
        .unwrap();
    let snapshot = service
        .runtime_snapshot(&fixture.context, model.id)
        .await
        .unwrap();
    let budget = LiveTransportRequestBudget::new(2);
    let mut templates = semantic_live_provider_templates("externalmodel", 30);
    if let Some(prompt_id) = fixture_settings.prompt_override {
        templates[0].prompt_id = prompt_id.to_owned();
    }
    let boundary = ProviderServiceAppBoundary::new(
        service,
        fixture.context.clone(),
        model.id,
        "externalmodel",
        templates.clone(),
        snapshot,
        budget,
    )
    .unwrap();
    (boundary, calls, expected_namespace, server, templates)
}

#[tokio::test]
async fn production_two_stage_empty_observation_completes_without_composition() {
    let temp = TempDir::new().unwrap();
    let baseline = fixture(temp.path(), "baseline-empty").await;
    let b = fixture(temp.path(), "arm-b-empty").await;
    let c = fixture(temp.path(), "arm-c-empty").await;
    let path = VaultPath::parse("notes/empty.md").unwrap();
    let bytes = b"# Empty\nNothing durable here.\n";
    let created = b
        .core
        .create_bytes(
            &b.context,
            &path,
            bytes,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let source = EvalSource {
        logical_id: "S-empty".into(),
        synthetic_placeholder: false,
        vault_id: b.context.id().to_string(),
        file_id: created.file.id.to_string(),
        path: path.as_str().into(),
        file_revision: created.file.current_revision.value(),
        content_hash: format!("{:x}", Sha256::digest(bytes)),
        logical_block_count: 0,
        source_revision_id: "pending".into(),
        authorization_revision: 1,
        profile_id: "semantic-profile-v1".into(),
        rules_revision: 1,
        source_generation: 1,
        extraction_commit_sequence: 1,
    };
    let (provider, calls, expected_namespace, server, templates) = provider_fixture(
        &b,
        true,
        false,
        false,
        false,
        ProviderKind::OpenAiCompatible,
        false,
    )
    .await;
    let semantic = SemanticMemoryServiceAppBoundary::new_isolated(
        baseline.state.clone(),
        baseline.context.clone(),
        baseline.db.clone(),
        baseline.history.clone(),
        runtime(&b),
        runtime(&c),
    )
    .unwrap();
    let provider = provider.with_prepared_extraction_cleanup(semantic.clone());
    let input = semantic
        .prepare_for_arm(ComparisonArm::B, &source)
        .await
        .unwrap();
    *expected_namespace.lock().unwrap() =
        Some(input["evidence_namespace"].as_str().unwrap().to_owned());
    let generated = provider
        .generate(LiveProviderRequest {
            sequence: 1,
            stage: "observation".into(),
            arm: ComparisonArm::B,
            source_id: Some(source.logical_id.clone()),
            task_id: None,
            model_id: "externalmodel".into(),
            prompt_id: templates[0].prompt_id.clone(),
            schema_id: templates[0].schema_id.clone(),
            index_profile_id: templates[0].index_profile_id.clone(),
            input,
        })
        .await
        .unwrap();
    let phase = semantic
        .accept_observation_for_arm(ComparisonArm::B, &source, &generated.output)
        .await
        .unwrap();
    assert_eq!(phase["status"], "success_empty");
    assert_eq!(phase["observations"].as_array().unwrap().len(), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let projection = semantic
        .current_card_projection_for_arm(ComparisonArm::B, &source)
        .await
        .unwrap();
    assert_eq!(projection["card_count"], 0);
    assert!(
        b.state
            .semantic_memory()
            .pending_extractions(&b.context)
            .await
            .unwrap()
            .is_empty()
    );
    server.abort();
}

#[tokio::test]
async fn mimo_v7_observation_uses_one_strict_function_json_response() {
    let temp = TempDir::new().unwrap();
    let fixture = fixture(temp.path(), "mimo-strict-function").await;
    let (provider, calls, expected_namespace, server, templates) = provider_fixture(
        &fixture,
        false,
        false,
        false,
        false,
        ProviderKind::XiaomiMimo,
        false,
    )
    .await;
    *expected_namespace.lock().unwrap() = Some("h-mimo-strict-function".to_owned());
    let observation = templates
        .iter()
        .find(|template| template.stage == "observation")
        .unwrap();
    assert_eq!(observation.prompt_id, "semantic-cards-tracked-adr-m6-v12");
    assert_eq!(observation.schema_id, "semantic-cards-m6-json-v8");
    let generated = provider
        .generate(LiveProviderRequest {
            sequence: 1,
            stage: "observation".into(),
            arm: ComparisonArm::B,
            source_id: Some("mimo-strict-function".into()),
            task_id: None,
            model_id: "externalmodel".into(),
            prompt_id: observation.prompt_id.clone(),
            schema_id: observation.schema_id.clone(),
            index_profile_id: observation.index_profile_id.clone(),
            input: json!({
                "evidence_namespace":"h-mimo-strict-function",
                "blocks":[{"evidence_index":1}]
            }),
        })
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        generated.output["evidence_namespace"],
        "h-mimo-strict-function"
    );
    assert_eq!(generated.output["outcome"], "success_nonempty");
    assert_eq!(
        generated.output["observations"][0]["source_time_scope"]["value"],
        ""
    );
    assert_eq!(
        generated.output["observations"][0]["context_block_indices"],
        json!([])
    );
    server.abort();
}

#[tokio::test]
async fn v7_mimo_body_context_overlap_is_normalized_before_composition_and_persisted_once() {
    let temp = TempDir::new().unwrap();
    let baseline = fixture(temp.path(), "overlap-baseline").await;
    let b = fixture(temp.path(), "overlap-arm-b").await;
    let c = fixture(temp.path(), "overlap-arm-c").await;
    let source = create_eval_source(
        &b,
        "S-overlap",
        "notes/overlap.md",
        b"Use reviewed changes and preserve the review scope.\nAdditional independent context.\n",
    )
    .await;
    let (provider, calls, expected_namespace, server, templates) = provider_fixture_with_settings(
        &b,
        ProviderFixtureSettings {
            empty: false,
            fail_second: false,
            invalid_schema: false,
            invalid_assertion_status: false,
            provider_kind: ProviderKind::XiaomiMimo,
            extra_observation_property: false,
            body_context_overlap: true,
            all_m6_stages: false,
            prompt_override: None,
            strict_reply: StrictReply::Valid,
        },
    )
    .await;
    let semantic = SemanticMemoryServiceAppBoundary::new_isolated(
        baseline.state.clone(),
        baseline.context.clone(),
        baseline.db.clone(),
        baseline.history.clone(),
        runtime(&b),
        runtime(&c),
    )
    .unwrap();
    let input = semantic
        .prepare_for_arm(ComparisonArm::B, &source)
        .await
        .unwrap();
    *expected_namespace.lock().unwrap() =
        Some(input["evidence_namespace"].as_str().unwrap().to_owned());
    let observation_template = templates
        .iter()
        .find(|template| template.stage == "observation")
        .unwrap();
    let generated_observation = provider
        .generate(LiveProviderRequest {
            sequence: 1,
            stage: "observation".into(),
            arm: ComparisonArm::B,
            source_id: Some(source.logical_id.clone()),
            task_id: None,
            model_id: "externalmodel".into(),
            prompt_id: observation_template.prompt_id.clone(),
            schema_id: observation_template.schema_id.clone(),
            index_profile_id: observation_template.index_profile_id.clone(),
            input,
        })
        .await
        .unwrap();
    let composition_input = semantic
        .accept_observation_for_arm(ComparisonArm::B, &source, &generated_observation.output)
        .await
        .unwrap();
    assert_eq!(
        composition_input["observations"][0]["context_block_ids"],
        json!([])
    );
    assert_eq!(
        composition_input["observations"][0]["body_block_ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        semantic
            .take_observation_context_overlap_removed_for_arm(ComparisonArm::B, &source)
            .await,
        1
    );

    let composition_template = templates
        .iter()
        .find(|template| template.stage == "composition")
        .unwrap();
    let generated_composition = provider
        .generate(LiveProviderRequest {
            sequence: 2,
            stage: "composition".into(),
            arm: ComparisonArm::B,
            source_id: Some(source.logical_id.clone()),
            task_id: None,
            model_id: "externalmodel".into(),
            prompt_id: composition_template.prompt_id.clone(),
            schema_id: composition_template.schema_id.clone(),
            index_profile_id: composition_template.index_profile_id.clone(),
            input: composition_input,
        })
        .await
        .unwrap();
    let submitted = semantic
        .submit_composition_for_arm(ComparisonArm::B, &source, &generated_composition.output)
        .await
        .unwrap();
    assert_eq!(submitted["state"], "success_nonempty");
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    let projection = semantic
        .current_card_projection_for_arm(ComparisonArm::B, &source)
        .await
        .unwrap();
    let evidence_id = EvidenceRefId::parse(
        projection["cards"][0]["assertions"][0]["evidence_refs"][0]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let evidence = b
        .state
        .semantic_memory()
        .get_evidence(&b.context, evidence_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(evidence.body_spans.len(), 1);
    assert!(evidence.context_spans.is_empty());
    server.abort();
}

#[tokio::test]
async fn versioned_mimo_m6_four_stages_use_strict_nonstream_and_relation_sentinels_validate_locally()
 {
    let temp = TempDir::new().unwrap();
    let fixture = fixture(temp.path(), "mimo-four-stage-strict").await;
    let (provider, calls, expected_namespace, server, templates) = provider_fixture_with_settings(
        &fixture,
        ProviderFixtureSettings {
            empty: false,
            fail_second: false,
            invalid_schema: false,
            invalid_assertion_status: false,
            provider_kind: ProviderKind::XiaomiMimo,
            extra_observation_property: false,
            body_context_overlap: false,
            all_m6_stages: true,
            prompt_override: None,
            strict_reply: StrictReply::Valid,
        },
    )
    .await;
    let namespace = "h-four-stage-source-revision";
    *expected_namespace.lock().unwrap() = Some(namespace.to_owned());
    assert_eq!(templates.len(), 4);
    for (sequence, template) in templates.iter().enumerate() {
        assert_eq!(template.prompt_id, "semantic-cards-tracked-adr-m6-v12");
        assert_eq!(template.schema_id, "semantic-cards-m6-json-v8");
        let input = match template.stage.as_str() {
            "observation" => json!({
                "evidence_namespace":namespace,
                "blocks":[
                    {"evidence_index":1,"text":"fixture evidence","line_number":1,"kind":"paragraph"}
                ]
            }),
            "composition" => json!({
                "observations":[{"statement":"fixture"}],"groups":[]
            }),
            "relation" => json!({"candidates":[{"candidate_id":"candidate-1"}]}),
            "answer" => json!({"task":"fixture question","memory_pack":[]}),
            _ => panic!("unexpected M6 stage"),
        };
        let generated = provider
            .generate(LiveProviderRequest {
                sequence: sequence as u32 + 1,
                stage: template.stage.clone(),
                arm: ComparisonArm::B,
                source_id: Some("S-stage-matrix".into()),
                task_id: None,
                model_id: "externalmodel".into(),
                prompt_id: template.prompt_id.clone(),
                schema_id: template.schema_id.clone(),
                index_profile_id: template.index_profile_id.clone(),
                input,
            })
            .await
            .unwrap();
        if template.stage == "relation" {
            let action = &generated.output["actions"][0];
            assert_eq!(action["action"], "no_change");
            assert_eq!(action["candidate_ids"], json!(["candidate-1"]));
            for field in [
                "card_ref",
                "title",
                "content",
                "item_kind",
                "support_operator",
                "reason",
            ] {
                assert!(
                    action.get(field).is_none(),
                    "empty {field} sentinel was not normalized"
                );
            }
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    server.abort();
}

#[tokio::test]
async fn production_observation_catalog_errors_before_any_provider_request() {
    let temp = TempDir::new().unwrap();
    let fixture = fixture(temp.path(), "catalog-invalid").await;
    let (provider, calls, _expected_namespace, server, templates) = provider_fixture(
        &fixture,
        false,
        false,
        false,
        false,
        ProviderKind::OpenAiCompatible,
        false,
    )
    .await;
    for input in [json!({}), json!({"blocks": []})] {
        let error = provider
            .generate(LiveProviderRequest {
                sequence: 1,
                stage: "observation".into(),
                arm: ComparisonArm::B,
                source_id: Some("catalog-invalid".into()),
                task_id: None,
                model_id: "externalmodel".into(),
                prompt_id: templates[0].prompt_id.clone(),
                schema_id: templates[0].schema_id.clone(),
                index_profile_id: templates[0].index_profile_id.clone(),
                input,
            })
            .await
            .unwrap_err();
        assert!(
            error.code == "semantic_observation_catalog_missing"
                || error.code == "semantic_observation_catalog_invalid"
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn provider_adapter_terminalizes_preaccept_observation_failure() {
    let temp = TempDir::new().unwrap();
    let baseline = fixture(temp.path(), "baseline-preaccept").await;
    let b = fixture(temp.path(), "arm-b-preaccept").await;
    let c = fixture(temp.path(), "arm-c-preaccept").await;
    let source = create_eval_source(
        &b,
        "S-preaccept",
        "notes/preaccept.md",
        b"# Preaccept\nThe dynamic catalog must be present.\n",
    )
    .await;
    let (provider, calls, _expected_namespace, server, templates) = provider_fixture(
        &b,
        false,
        false,
        false,
        false,
        ProviderKind::OpenAiCompatible,
        false,
    )
    .await;
    let semantic = SemanticMemoryServiceAppBoundary::new_isolated(
        baseline.state.clone(),
        baseline.context.clone(),
        baseline.db.clone(),
        baseline.history.clone(),
        runtime(&b),
        runtime(&c),
    )
    .unwrap();
    let provider = provider.with_prepared_extraction_cleanup(semantic.clone());
    let _input = semantic
        .prepare_for_arm(ComparisonArm::B, &source)
        .await
        .unwrap();
    let (extraction_id, state) = latest_extraction(&b).await;
    assert_eq!(state, "running");

    let error = provider
        .generate(LiveProviderRequest {
            sequence: 1,
            stage: "observation".into(),
            arm: ComparisonArm::B,
            source_id: Some(source.logical_id.clone()),
            task_id: None,
            model_id: "externalmodel".into(),
            prompt_id: templates[0].prompt_id.clone(),
            schema_id: templates[0].schema_id.clone(),
            index_profile_id: templates[0].index_profile_id.clone(),
            input: json!({"blocks": []}),
        })
        .await
        .unwrap_err();

    assert_eq!(error.code, "semantic_observation_catalog_invalid");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let extraction = b
        .state
        .semantic_memory()
        .get_extraction(&b.context, extraction_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(extraction.state, "cancelled");
    assert_eq!(running_extraction_count(&b).await, 0);
    assert!(
        b.state
            .semantic_memory()
            .pending_extractions(&b.context)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        b.state
            .semantic_memory()
            .list_cards(&b.context, 20)
            .await
            .unwrap()
            .is_empty()
    );
    server.abort();
}

#[tokio::test]
async fn strict_mimo_no_tool_failure_keeps_only_safe_protocol_issue_and_aborts() {
    let temp = TempDir::new().unwrap();
    let baseline = fixture(temp.path(), "baseline-strict-no-tool").await;
    let b = fixture(temp.path(), "arm-b-strict-no-tool").await;
    let c = fixture(temp.path(), "arm-c-strict-no-tool").await;
    let source = create_eval_source(
        &b,
        "S-strict-no-tool",
        "notes/strict-no-tool.md",
        b"# Strict function fixture\nKeep no-tool failures bounded.\n",
    )
    .await;
    let (provider, calls, expected_namespace, server, templates) = provider_fixture_with_settings(
        &b,
        ProviderFixtureSettings {
            empty: false,
            fail_second: false,
            invalid_schema: false,
            invalid_assertion_status: false,
            provider_kind: ProviderKind::XiaomiMimo,
            extra_observation_property: false,
            body_context_overlap: false,
            all_m6_stages: false,
            prompt_override: None,
            strict_reply: StrictReply::NoTool,
        },
    )
    .await;
    let semantic = SemanticMemoryServiceAppBoundary::new_isolated(
        baseline.state.clone(),
        baseline.context.clone(),
        baseline.db.clone(),
        baseline.history.clone(),
        runtime(&b),
        runtime(&c),
    )
    .unwrap();
    let provider = provider.with_prepared_extraction_cleanup(semantic.clone());
    let input = semantic
        .prepare_for_arm(ComparisonArm::B, &source)
        .await
        .unwrap();
    *expected_namespace.lock().unwrap() =
        Some(input["evidence_namespace"].as_str().unwrap().to_owned());
    let (extraction_id, state) = latest_extraction(&b).await;
    assert_eq!(state, "running");

    let error = provider
        .generate(LiveProviderRequest {
            sequence: 1,
            stage: "observation".into(),
            arm: ComparisonArm::B,
            source_id: Some(source.logical_id.clone()),
            task_id: None,
            model_id: "externalmodel".into(),
            prompt_id: templates[0].prompt_id.clone(),
            schema_id: templates[0].schema_id.clone(),
            index_profile_id: templates[0].index_profile_id.clone(),
            input,
        })
        .await
        .unwrap_err();
    server.abort();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(error.code, "provider_response_invalid");
    assert_eq!(
        error.protocol_issue,
        Some(mcp_vault_providers::StrictFunctionCallIssue::NoToolCall)
    );
    assert_eq!(error.schema_issue, None);
    assert_eq!(error.schema_path, None);
    let diagnostic = json!({"code":error.code,"protocol_issue":error.protocol_issue});
    assert!(!diagnostic.to_string().contains("arguments"));
    let extraction = b
        .state
        .semantic_memory()
        .get_extraction(&b.context, extraction_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(extraction.state, "cancelled");
    assert_eq!(running_extraction_count(&b).await, 0);
    assert!(
        b.state
            .semantic_memory()
            .list_cards(&b.context, 20)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn strict_mimo_nonstream_tool_responses_fail_closed_with_safe_categories() {
    use mcp_vault_providers::StrictFunctionCallIssue;

    let cases = [
        (
            StrictReply::NoTool,
            Some(StrictFunctionCallIssue::NoToolCall),
            "provider_response_invalid",
        ),
        (
            StrictReply::MultipleTools,
            Some(StrictFunctionCallIssue::MultipleToolCalls),
            "provider_response_invalid",
        ),
        (
            StrictReply::WrongName,
            Some(StrictFunctionCallIssue::WrongToolName),
            "provider_response_invalid",
        ),
        (
            StrictReply::InvalidArguments,
            None,
            "provider_structured_json_invalid",
        ),
        (
            StrictReply::MixedContent,
            Some(StrictFunctionCallIssue::MixedMessageContent),
            "provider_response_invalid",
        ),
        (StrictReply::TooLarge, None, "provider_response_too_large"),
        (
            StrictReply::WrongFinish,
            Some(StrictFunctionCallIssue::WrongFinishReason),
            "provider_response_invalid",
        ),
        (
            StrictReply::MissingId,
            Some(StrictFunctionCallIssue::MissingToolCallId),
            "provider_response_invalid",
        ),
    ];
    for (index, (strict_reply, expected_issue, expected_code)) in cases.into_iter().enumerate() {
        let temp = TempDir::new().unwrap();
        let fixture = fixture(temp.path(), &format!("strict-json-reply-{index}")).await;
        let (provider, calls, expected_namespace, server, templates) =
            provider_fixture_with_settings(
                &fixture,
                ProviderFixtureSettings {
                    empty: false,
                    fail_second: false,
                    invalid_schema: false,
                    invalid_assertion_status: false,
                    provider_kind: ProviderKind::XiaomiMimo,
                    extra_observation_property: false,
                    body_context_overlap: false,
                    all_m6_stages: false,
                    prompt_override: None,
                    strict_reply,
                },
            )
            .await;
        *expected_namespace.lock().unwrap() = Some("h-strict-json-case".to_owned());
        let error = provider
            .generate(LiveProviderRequest {
                sequence: 1,
                stage: "observation".into(),
                arm: ComparisonArm::B,
                source_id: Some(format!("S-case-{index}")),
                task_id: None,
                model_id: "externalmodel".into(),
                prompt_id: templates[0].prompt_id.clone(),
                schema_id: templates[0].schema_id.clone(),
                index_profile_id: templates[0].index_profile_id.clone(),
                input: json!({
                    "evidence_namespace":"h-strict-json-case",
                    "blocks":[{"evidence_index":1}]
                }),
            })
            .await
            .unwrap_err();
        server.abort();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(error.code, expected_code);
        assert_eq!(error.protocol_issue, expected_issue);
        assert_eq!(error.schema_issue, None);
        assert_eq!(error.schema_path, None);
        assert!(!format!("{error:?}").contains("arguments"));
    }
}

#[tokio::test]
async fn production_provider_boundary_reports_resolved_schema_error() {
    let temp = TempDir::new().unwrap();
    let fixture = fixture(temp.path(), "schema-invalid").await;
    let (provider, calls, expected_namespace, server, templates) = provider_fixture(
        &fixture,
        false,
        false,
        true,
        false,
        ProviderKind::OpenAiCompatible,
        false,
    )
    .await;
    let input = json!({
        "evidence_namespace":"h-schema-invalid",
        "blocks": [
            {"evidence_index": 1},
            {"evidence_index": 2}
        ]
    });
    *expected_namespace.lock().unwrap() = Some("h-schema-invalid".to_owned());
    let error = provider
        .generate(LiveProviderRequest {
            sequence: 1,
            stage: "observation".into(),
            arm: ComparisonArm::B,
            source_id: Some("schema-invalid".into()),
            task_id: None,
            model_id: "externalmodel".into(),
            prompt_id: templates[0].prompt_id.clone(),
            schema_id: templates[0].schema_id.clone(),
            index_profile_id: templates[0].index_profile_id.clone(),
            input,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "provider_schema_invalid");
    assert_eq!(error.schema_issue.as_deref(), Some("enum_mismatch"));
    assert_eq!(
        error.schema_path.as_deref(),
        Some("$.observations[0].body_block_indices[0]")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_ne!(
        error.schema_path.as_deref(),
        Some("fixture-forbidden-block")
    );
    server.abort();
}

#[tokio::test]
async fn provider_boundary_rejects_noncanonical_assertion_status_enum() {
    let temp = TempDir::new().unwrap();
    let fixture = fixture(temp.path(), "status-enum-invalid").await;
    let (provider, calls, expected_namespace, server, templates) = provider_fixture(
        &fixture,
        false,
        false,
        false,
        true,
        ProviderKind::XiaomiMimo,
        false,
    )
    .await;
    *expected_namespace.lock().unwrap() = Some("h-status-enum-invalid".to_owned());
    let error = provider
        .generate(LiveProviderRequest {
            sequence: 1,
            stage: "observation".into(),
            arm: ComparisonArm::B,
            source_id: Some("status-enum-invalid".into()),
            task_id: None,
            model_id: "externalmodel".into(),
            prompt_id: templates[0].prompt_id.clone(),
            schema_id: templates[0].schema_id.clone(),
            index_profile_id: templates[0].index_profile_id.clone(),
            input: json!({
                "evidence_namespace":"h-status-enum-invalid",
                "blocks":[{"evidence_index":1}]
            }),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "provider_schema_invalid");
    assert_eq!(error.schema_issue.as_deref(), Some("enum_mismatch"));
    assert_eq!(
        error.schema_path.as_deref(),
        Some("$.observations[0].assertion_status")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn mimo_json_object_compatibility_and_strict_function_fail_closed() {
    let temp = TempDir::new().unwrap();
    let mimo_fixture = fixture(temp.path(), "mimo-contract").await;
    let (provider, calls, expected_namespace, server, templates) = provider_fixture_with_settings(
        &mimo_fixture,
        ProviderFixtureSettings {
            empty: false,
            fail_second: false,
            invalid_schema: false,
            invalid_assertion_status: false,
            provider_kind: ProviderKind::XiaomiMimo,
            extra_observation_property: false,
            body_context_overlap: false,
            all_m6_stages: false,
            prompt_override: Some("mimo-json-object-compat-test"),
            strict_reply: StrictReply::Valid,
        },
    )
    .await;
    let input = json!({
        "evidence_namespace":"h-mimo-contract",
        "blocks": [
            {"evidence_index":1},
            {"evidence_index":2}
        ]
    });
    *expected_namespace.lock().unwrap() = Some("h-mimo-contract".into());
    let request = LiveProviderRequest {
        sequence: 1,
        stage: "observation".into(),
        arm: ComparisonArm::B,
        source_id: Some("mimo-contract".into()),
        task_id: None,
        model_id: "externalmodel".into(),
        prompt_id: templates[0].prompt_id.clone(),
        schema_id: templates[0].schema_id.clone(),
        index_profile_id: templates[0].index_profile_id.clone(),
        input,
    };
    let generated = provider.generate(request).await.unwrap();
    assert_eq!(generated.output["outcome"], "success_nonempty");
    assert_eq!(
        generated.output["observations"].as_array().unwrap().len(),
        2
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.abort();

    let extra_fixture = fixture(temp.path(), "mimo-extra-property").await;
    let (provider, calls, expected_namespace, server, templates) = provider_fixture(
        &extra_fixture,
        false,
        false,
        false,
        false,
        ProviderKind::XiaomiMimo,
        true,
    )
    .await;
    *expected_namespace.lock().unwrap() = Some("h-mimo-extra".into());
    let error = provider
        .generate(LiveProviderRequest {
            sequence: 1,
            stage: "observation".into(),
            arm: ComparisonArm::C,
            source_id: Some("mimo-extra-property".into()),
            task_id: None,
            model_id: "externalmodel".into(),
            prompt_id: templates[0].prompt_id.clone(),
            schema_id: templates[0].schema_id.clone(),
            index_profile_id: templates[0].index_profile_id.clone(),
            input: json!({"evidence_namespace":"h-mimo-extra","blocks":[{"evidence_index":1}]}),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "provider_schema_invalid");
    assert_eq!(error.schema_issue.as_deref(), Some("unexpected_property"));
    assert_eq!(error.schema_path.as_deref(), Some("$.observations[0]"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn production_two_stage_composition_error_can_abort_running_extraction() {
    let temp = TempDir::new().unwrap();
    let baseline = fixture(temp.path(), "baseline-error").await;
    let b = fixture(temp.path(), "arm-b-error").await;
    let c = fixture(temp.path(), "arm-c-error").await;
    let path = VaultPath::parse("notes/error.md").unwrap();
    let bytes = b"# Error\nUse reviewed changes.\nKeep the boundary isolated.\n";
    let created = b
        .core
        .create_bytes(
            &b.context,
            &path,
            bytes,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let source = EvalSource {
        logical_id: "S-error".into(),
        synthetic_placeholder: false,
        vault_id: b.context.id().to_string(),
        file_id: created.file.id.to_string(),
        path: path.as_str().into(),
        file_revision: created.file.current_revision.value(),
        content_hash: format!("{:x}", Sha256::digest(bytes)),
        logical_block_count: 0,
        source_revision_id: "pending".into(),
        authorization_revision: 1,
        profile_id: "semantic-profile-v1".into(),
        rules_revision: 1,
        source_generation: 1,
        extraction_commit_sequence: 1,
    };
    let (provider, calls, expected_namespace, server, templates) = provider_fixture(
        &b,
        false,
        true,
        false,
        false,
        ProviderKind::OpenAiCompatible,
        false,
    )
    .await;
    let semantic = SemanticMemoryServiceAppBoundary::new_isolated(
        baseline.state.clone(),
        baseline.context.clone(),
        baseline.db.clone(),
        baseline.history.clone(),
        runtime(&b),
        runtime(&c),
    )
    .unwrap();
    let provider = provider.with_prepared_extraction_cleanup(semantic.clone());
    let input = semantic
        .prepare_for_arm(ComparisonArm::B, &source)
        .await
        .unwrap();
    *expected_namespace.lock().unwrap() =
        Some(input["evidence_namespace"].as_str().unwrap().to_owned());
    let observation = provider
        .generate(LiveProviderRequest {
            sequence: 1,
            stage: "observation".into(),
            arm: ComparisonArm::B,
            source_id: Some(source.logical_id.clone()),
            task_id: None,
            model_id: "externalmodel".into(),
            prompt_id: templates[0].prompt_id.clone(),
            schema_id: templates[0].schema_id.clone(),
            index_profile_id: templates[0].index_profile_id.clone(),
            input,
        })
        .await
        .unwrap();
    let composition_input = semantic
        .accept_observation_for_arm(ComparisonArm::B, &source, &observation.output)
        .await
        .unwrap();
    let error = provider
        .generate(LiveProviderRequest {
            sequence: 2,
            stage: "composition".into(),
            arm: ComparisonArm::B,
            source_id: Some(source.logical_id.clone()),
            task_id: None,
            model_id: "externalmodel".into(),
            prompt_id: templates[1].prompt_id.clone(),
            schema_id: templates[1].schema_id.clone(),
            index_profile_id: templates[1].index_profile_id.clone(),
            input: composition_input,
        })
        .await
        .unwrap_err();
    assert!(!error.code.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let projection = semantic
        .current_card_projection_for_arm(ComparisonArm::B, &source)
        .await;
    assert!(projection.is_err());
    assert!(
        b.state
            .semantic_memory()
            .pending_extractions(&b.context)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(running_extraction_count(&b).await, 0);
    server.abort();
}

#[tokio::test]
async fn production_observation_schema_error_aborts_prepared_extraction() {
    let temp = TempDir::new().unwrap();
    let baseline = fixture(temp.path(), "baseline-observation-error").await;
    let b = fixture(temp.path(), "arm-b-observation-error").await;
    let c = fixture(temp.path(), "arm-c-observation-error").await;
    let path = VaultPath::parse("notes/observation-error.md").unwrap();
    let bytes = b"# Observation error\nThe schema must reject a forbidden block.\n";
    let created = b
        .core
        .create_bytes(
            &b.context,
            &path,
            bytes,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let source = EvalSource {
        logical_id: "S-observation-error".into(),
        synthetic_placeholder: false,
        vault_id: b.context.id().to_string(),
        file_id: created.file.id.to_string(),
        path: path.as_str().into(),
        file_revision: created.file.current_revision.value(),
        content_hash: format!("{:x}", Sha256::digest(bytes)),
        logical_block_count: 0,
        source_revision_id: "pending".into(),
        authorization_revision: 1,
        profile_id: "semantic-profile-v1".into(),
        rules_revision: 1,
        source_generation: 1,
        extraction_commit_sequence: 1,
    };
    let (provider, calls, expected_namespace, server, templates) = provider_fixture(
        &b,
        false,
        false,
        true,
        false,
        ProviderKind::OpenAiCompatible,
        false,
    )
    .await;
    let semantic = SemanticMemoryServiceAppBoundary::new_isolated(
        baseline.state.clone(),
        baseline.context.clone(),
        baseline.db.clone(),
        baseline.history.clone(),
        runtime(&b),
        runtime(&c),
    )
    .unwrap();
    let provider = provider.with_prepared_extraction_cleanup(semantic.clone());
    let input = semantic
        .prepare_for_arm(ComparisonArm::B, &source)
        .await
        .unwrap();
    *expected_namespace.lock().unwrap() =
        Some(input["evidence_namespace"].as_str().unwrap().to_owned());
    let error = provider
        .generate(LiveProviderRequest {
            sequence: 1,
            stage: "observation".into(),
            arm: ComparisonArm::B,
            source_id: Some(source.logical_id.clone()),
            task_id: None,
            model_id: "externalmodel".into(),
            prompt_id: templates[0].prompt_id.clone(),
            schema_id: templates[0].schema_id.clone(),
            index_profile_id: templates[0].index_profile_id.clone(),
            input,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "provider_schema_invalid");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let (extraction_id, _) = latest_extraction(&b).await;
    let extraction = b
        .state
        .semantic_memory()
        .get_extraction(&b.context, extraction_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(extraction.state, "cancelled");
    assert_eq!(running_extraction_count(&b).await, 0);
    assert!(
        b.state
            .semantic_memory()
            .pending_extractions(&b.context)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        b.state
            .semantic_memory()
            .list_cards(&b.context, 20)
            .await
            .unwrap()
            .is_empty()
    );
    server.abort();
}

#[tokio::test]
async fn provider_service_sse_structured_json_failure_is_redacted_and_aborted() {
    let temp = TempDir::new().unwrap();
    let baseline = fixture(temp.path(), "baseline-json-error").await;
    let b = fixture(temp.path(), "arm-b-json-error").await;
    let c = fixture(temp.path(), "arm-c-json-error").await;
    let path = VaultPath::parse("notes/json-error.md").unwrap();
    let bytes = b"# JSON error\nThe output must stay private.\n";
    let created = b
        .core
        .create_bytes(
            &b.context,
            &path,
            bytes,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let source = EvalSource {
        logical_id: "S-json-error".into(),
        synthetic_placeholder: false,
        vault_id: b.context.id().to_string(),
        file_id: created.file.id.to_string(),
        path: path.as_str().into(),
        file_revision: created.file.current_revision.value(),
        content_hash: format!("{:x}", Sha256::digest(bytes)),
        logical_block_count: 0,
        source_revision_id: "pending".into(),
        authorization_revision: 1,
        profile_id: "semantic-profile-v1".into(),
        rules_revision: 1,
        source_generation: 1,
        extraction_commit_sequence: 1,
    };

    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/v1/chat/completions", post(malformed_json_chat))
        .with_state(MalformedJsonState {
            calls: calls.clone(),
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let auth = AuthService::new(
        b.state.auth(),
        MasterKeyRing::from_bytes(1, &[13; 32]).unwrap(),
    );
    let service = ProviderService::new(b.state.clone(), auth);
    service
        .set_provider_mode(&b.context, ProviderMode::LocalOnly, None)
        .await
        .unwrap();
    let provider = service
        .create_provider(ProviderInput {
            name: "malformed SSE fixture".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: Url::parse(&format!("http://{addr}/v1")).unwrap(),
            settings: ProviderSettings {
                max_retries: 0,
                ..ProviderSettings::default()
            },
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();
    let model = service
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "externalmodel".into(),
            capabilities: ModelCapabilities {
                structured_output: true,
                ..Default::default()
            },
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await
        .unwrap();
    let snapshot = service
        .runtime_snapshot(&b.context, model.id)
        .await
        .unwrap();
    let templates = semantic_live_provider_templates("externalmodel", 30);
    let provider_boundary = ProviderServiceAppBoundary::new(
        service,
        b.context.clone(),
        model.id,
        "externalmodel",
        templates.clone(),
        snapshot,
        LiveTransportRequestBudget::new(2),
    )
    .unwrap();
    let semantic = SemanticMemoryServiceAppBoundary::new_isolated(
        baseline.state.clone(),
        baseline.context.clone(),
        baseline.db.clone(),
        baseline.history.clone(),
        runtime(&b),
        runtime(&c),
    )
    .unwrap();
    let provider_boundary = provider_boundary.with_prepared_extraction_cleanup(semantic.clone());
    let input = semantic
        .prepare_for_arm(ComparisonArm::B, &source)
        .await
        .unwrap();

    let (extraction_id, state) = latest_extraction(&b).await;
    assert_eq!(state, "running");

    let log_bytes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log_sink = log_bytes.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(move || CapturedLogWriter(log_sink.clone()))
        .finish();
    let request = LiveProviderRequest {
        sequence: 1,
        stage: "observation".into(),
        arm: ComparisonArm::B,
        source_id: Some(source.logical_id.clone()),
        task_id: None,
        model_id: "externalmodel".into(),
        prompt_id: templates[0].prompt_id.clone(),
        schema_id: templates[0].schema_id.clone(),
        index_profile_id: templates[0].index_profile_id.clone(),
        input,
    };
    let error = provider_boundary
        .generate(request)
        .with_subscriber(subscriber)
        .await
        .unwrap_err();
    server.abort();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(error.code, "provider_structured_json_invalid");
    assert_eq!(error.schema_issue, None);
    assert_eq!(error.schema_path, None);
    let diagnostic = error.structured_json_diagnostic.as_ref().unwrap();
    assert_eq!(
        diagnostic.parser_category,
        StructuredJsonParserCategory::Syntax
    );
    assert_eq!(diagnostic.issue, StructuredJsonParseIssue::TrailingComma);
    assert_eq!(diagnostic.line, 1);
    assert_eq!(diagnostic.column, 42);
    assert_eq!(diagnostic.content_bytes, 42);
    assert_eq!(diagnostic.parsed_bytes, 42);
    assert!(!diagnostic.fence_detected);
    assert_eq!(
        diagnostic.finish_reason,
        Some(StructuredJsonFinishReason::Stop)
    );

    let logs = String::from_utf8(log_bytes.lock().unwrap().clone()).unwrap();
    assert!(!logs.contains(PRIVATE_JSON_MARKER));
    let report = json!({
        "code": error.code,
        "schema_issue": error.schema_issue,
        "schema_path": error.schema_path,
        "structured_json_diagnostic": diagnostic,
    })
    .to_string();
    assert!(!report.contains(PRIVATE_JSON_MARKER));
    assert!(!format!("{error:?}").contains(PRIVATE_JSON_MARKER));

    let extraction = b
        .state
        .semantic_memory()
        .get_extraction(&b.context, extraction_id)
        .await
        .unwrap()
        .unwrap();
    let pending = b
        .state
        .semantic_memory()
        .pending_extractions(&b.context)
        .await
        .unwrap();
    let cards = b
        .state
        .semantic_memory()
        .list_cards(&b.context, 20)
        .await
        .unwrap();
    assert!(pending.is_empty());
    assert!(cards.is_empty());
    assert_eq!(extraction.state, "cancelled");
    assert_eq!(running_extraction_count(&b).await, 0);
}

#[tokio::test]
async fn production_two_stage_live_boundary_publishes_cards() {
    let temp = TempDir::new().unwrap();
    let baseline = fixture(temp.path(), "baseline").await;
    let b = fixture(temp.path(), "arm-b").await;
    let c = fixture(temp.path(), "arm-c").await;
    let path = VaultPath::parse("notes/live.md").unwrap();
    let bytes = b"# Boundary\nUse reviewed changes.\nKeep the boundary isolated.\n";
    let created = b
        .core
        .create_bytes(
            &b.context,
            &path,
            bytes,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let hash = format!("{:x}", Sha256::digest(bytes));
    let source = EvalSource {
        logical_id: "S-live".into(),
        synthetic_placeholder: false,
        vault_id: b.context.id().to_string(),
        file_id: created.file.id.to_string(),
        path: path.as_str().into(),
        file_revision: created.file.current_revision.value(),
        content_hash: hash.clone(),
        logical_block_count: 0,
        source_revision_id: "pending".into(),
        authorization_revision: 1,
        profile_id: "semantic-profile-v1".into(),
        rules_revision: 1,
        source_generation: 1,
        extraction_commit_sequence: 1,
    };

    let calls = Arc::new(AtomicUsize::new(0));
    let expected_namespace = Arc::new(std::sync::Mutex::new(None));
    let app = Router::new()
        .route("/v1/chat/completions", post(fake_chat))
        .with_state(FakeState {
            calls: calls.clone(),
            expected_namespace: expected_namespace.clone(),
            empty: false,
            fail_second: false,
            invalid_schema: false,
            invalid_assertion_status: false,
            extra_observation_property: false,
            body_context_overlap: false,
            all_m6_stages: false,
            expected_json_object: false,
            strict_reply: StrictReply::Valid,
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let auth = AuthService::new(
        b.state.auth(),
        MasterKeyRing::from_bytes(1, &[7; 32]).unwrap(),
    );
    let service = ProviderService::new(b.state.clone(), auth);
    service
        .set_provider_mode(&b.context, ProviderMode::LocalOnly, None)
        .await
        .unwrap();
    let provider = service
        .create_provider(ProviderInput {
            name: "fixture".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: Url::parse(&format!("http://{addr}/v1")).unwrap(),
            settings: ProviderSettings::default(),
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();
    let model = service
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "externalmodel".into(),
            capabilities: ModelCapabilities {
                structured_output: true,
                ..Default::default()
            },
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await
        .unwrap();
    let snapshot = service
        .runtime_snapshot(&b.context, model.id)
        .await
        .unwrap();
    let budget = LiveTransportRequestBudget::new(2);
    let templates = semantic_live_provider_templates("externalmodel", 30);
    let provider_boundary = ProviderServiceAppBoundary::new(
        service,
        b.context.clone(),
        model.id,
        "externalmodel",
        templates.clone(),
        snapshot,
        budget.clone(),
    )
    .unwrap();
    let semantic = SemanticMemoryServiceAppBoundary::new_isolated(
        baseline.state.clone(),
        baseline.context.clone(),
        baseline.db.clone(),
        baseline.history.clone(),
        runtime(&b),
        runtime(&c),
    )
    .unwrap();

    let input = semantic
        .prepare_for_arm(ComparisonArm::B, &source)
        .await
        .unwrap();
    *expected_namespace.lock().unwrap() =
        Some(input["evidence_namespace"].as_str().unwrap().to_owned());
    let req = LiveProviderRequest {
        sequence: 1,
        stage: "observation".into(),
        arm: ComparisonArm::B,
        source_id: Some(source.logical_id.clone()),
        task_id: None,
        model_id: "externalmodel".into(),
        prompt_id: templates[0].prompt_id.clone(),
        schema_id: templates[0].schema_id.clone(),
        index_profile_id: templates[0].index_profile_id.clone(),
        input,
    };
    let generated = provider_boundary.generate(req).await.unwrap();
    let composition_input = semantic
        .accept_observation_for_arm(ComparisonArm::B, &source, &generated.output)
        .await
        .unwrap();
    let req = LiveProviderRequest {
        sequence: 2,
        stage: "composition".into(),
        arm: ComparisonArm::B,
        source_id: Some(source.logical_id.clone()),
        task_id: None,
        model_id: "externalmodel".into(),
        prompt_id: templates[1].prompt_id.clone(),
        schema_id: templates[1].schema_id.clone(),
        index_profile_id: templates[1].index_profile_id.clone(),
        input: composition_input,
    };
    let generated = provider_boundary.generate(req).await.unwrap();
    let submitted = semantic
        .submit_composition_for_arm(ComparisonArm::B, &source, &generated.output)
        .await
        .unwrap();
    let projection = semantic
        .current_card_projection_for_arm(ComparisonArm::B, &source)
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(submitted["state"], "success_nonempty");
    assert_eq!(submitted["card_count"], 2);
    assert_eq!(projection["card_count"], 2);
    let cards = projection["cards"].as_array().unwrap();
    assert!(
        cards
            .iter()
            .any(|card| card["kind"] == "preference" && card["title"] == "Reviewed changes")
    );
    assert!(
        cards
            .iter()
            .any(|card| card["kind"] == "constraint" && card["title"] == "Isolated boundary")
    );
    let extraction_id =
        ExtractionSetId::parse(submitted["extraction_set_id"].as_str().unwrap()).unwrap();
    let extraction = b
        .state
        .semantic_memory()
        .get_extraction(&b.context, extraction_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(extraction.state, "success_nonempty");
    assert_eq!(extraction.observation_count, 2);
    assert_eq!(extraction.card_count, 2);
    assert_eq!(
        b.state
            .semantic_memory()
            .list_observations(&b.context, extraction.id)
            .await
            .unwrap()
            .len(),
        2
    );
    server.abort();
}

#[tokio::test]
async fn a80_fake_provider_batches_then_publishes_one_card_per_claim() {
    let temp = TempDir::new().unwrap();
    let baseline = fixture(temp.path(), "a80-baseline").await;
    let b = fixture(temp.path(), "a80-b").await;
    let c = fixture(temp.path(), "a80-c").await;
    let path = VaultPath::parse("notes/a80.md").unwrap();
    let mut bytes = String::from("# A80 source\n");
    for index in 0..85 {
        bytes.push_str(&format!("Supported source statement {index}.\n"));
    }
    let created = b
        .core
        .create_bytes(
            &b.context,
            &path,
            bytes.as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let source = EvalSource {
        logical_id: "S-a80".into(),
        synthetic_placeholder: false,
        vault_id: b.context.id().to_string(),
        file_id: created.file.id.to_string(),
        path: path.as_str().into(),
        file_revision: created.file.current_revision.value(),
        content_hash: format!("{:x}", Sha256::digest(bytes.as_bytes())),
        logical_block_count: u32::try_from(
            bytes.lines().filter(|line| !line.trim().is_empty()).count(),
        )
        .unwrap(),
        source_revision_id: "pending".into(),
        authorization_revision: 1,
        profile_id: "semantic-memory-m1-v1".into(),
        rules_revision: 1,
        source_generation: 1,
        extraction_commit_sequence: 1,
    };

    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/v1/chat/completions", post(a80_fake_chat))
        .with_state(A80FakeState {
            calls: calls.clone(),
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let auth = AuthService::new(
        b.state.auth(),
        MasterKeyRing::from_bytes(1, &[8; 32]).unwrap(),
    );
    let provider = ProviderService::new(b.state.clone(), auth);
    provider
        .set_provider_mode(&b.context, ProviderMode::LocalOnly, None)
        .await
        .unwrap();
    let configured = provider
        .create_provider(ProviderInput {
            name: "fixture".into(),
            kind: ProviderKind::XiaomiMimo,
            base_url: Url::parse(&format!("http://{addr}/v1")).unwrap(),
            settings: ProviderSettings::default(),
            enabled: true,
            secret: Some(SecretString::new("fixture-api-key-not-real")),
        })
        .await
        .unwrap();
    let model = provider
        .register_model(ModelInput {
            provider_id: configured.id,
            external_model_id: "externalmodel".into(),
            capabilities: ModelCapabilities {
                structured_output: true,
                ..Default::default()
            },
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await
        .unwrap();
    let snapshot = provider
        .runtime_snapshot(&b.context, model.id)
        .await
        .unwrap();
    let templates = semantic_a80_provider_templates("externalmodel", 30);
    assert_eq!(templates.len(), 3);
    let provider_boundary = ProviderServiceAppBoundary::new(
        provider,
        b.context.clone(),
        model.id,
        "externalmodel",
        templates.clone(),
        snapshot,
        LiveTransportRequestBudget::new(4),
    )
    .unwrap();
    let semantic = SemanticMemoryServiceAppBoundary::new_isolated(
        baseline.state.clone(),
        baseline.context.clone(),
        baseline.db.clone(),
        baseline.history.clone(),
        runtime(&b),
        runtime(&c),
    )
    .unwrap();
    let prepared = semantic
        .prepare_a80_for_arm(
            ComparisonArm::B,
            &source,
            "frozen-provider-fingerprint",
            &templates[0].prompt_id,
            &templates[0].schema_id,
        )
        .await
        .unwrap();
    let batches = prepared["batches"].as_array().unwrap();
    assert_eq!(batches.len(), 2);
    for batch in batches {
        let index = u32::try_from(batch["batch_index"].as_u64().unwrap()).unwrap();
        assert_eq!(
            semantic
                .reserve_a80_batch_for_arm(ComparisonArm::B, &source, index)
                .await
                .unwrap(),
            "dispatching"
        );
        let generated = provider_boundary
            .generate(LiveProviderRequest {
                sequence: index + 1,
                stage: "observation".into(),
                arm: ComparisonArm::B,
                source_id: Some(source.logical_id.clone()),
                task_id: None,
                model_id: "externalmodel".into(),
                prompt_id: templates[0].prompt_id.clone(),
                schema_id: templates[0].schema_id.clone(),
                index_profile_id: templates[0].index_profile_id.clone(),
                input: batch["input"].clone(),
            })
            .await
            .unwrap();
        semantic
            .accept_a80_batch_for_arm(ComparisonArm::B, &source, index, &generated.output)
            .await
            .unwrap();
    }
    let finalized = semantic
        .finalize_a80_for_arm(ComparisonArm::B, &source)
        .await
        .unwrap();
    assert_eq!(finalized["state"], "success_nonempty");
    assert_eq!(finalized["observation_count"], 2);
    assert_eq!(finalized["card_count"], 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // Recreate the Eval adapter after durable publication but before the
    // runner writes its card artifact. It must restore the published cards
    // from the same validated extraction without calling the Provider again.
    let restarted = SemanticMemoryServiceAppBoundary::new_isolated(
        baseline.state.clone(),
        baseline.context.clone(),
        baseline.db.clone(),
        baseline.history.clone(),
        runtime(&b),
        runtime(&c),
    )
    .unwrap();
    let resumed = restarted
        .prepare_a80_for_arm(
            ComparisonArm::B,
            &source,
            "frozen-provider-fingerprint",
            &templates[0].prompt_id,
            &templates[0].schema_id,
        )
        .await
        .unwrap();
    assert!(
        resumed["batches"]
            .as_array()
            .unwrap()
            .iter()
            .all(|batch| batch["batch_state"] == "validated")
    );
    let restored = restarted
        .finalize_a80_for_arm(ComparisonArm::B, &source)
        .await
        .unwrap();
    assert_eq!(restored["state"], "success_nonempty");
    let projection = restarted
        .current_card_projection_for_arm(ComparisonArm::B, &source)
        .await
        .unwrap();
    assert_eq!(projection["card_count"], 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let _ = provider_boundary;
    server.abort();
}
