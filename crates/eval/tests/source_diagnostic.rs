use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use mcp_vault_eval::{
    AuthoritativeSourceSnapshot, Budget, ComparisonArm, ComparisonConfig, DiagnosticSelection,
    EvalError, EvalMode, EvalSource, EvalTask, EvaluationManifest, EvaluationRunConfig,
    ExpectedRelation, LIVE_RUN_SCHEMA, LiveEvaluationConfig, LiveProviderError, LiveProviderOutput,
    LiveProviderRequest, ProviderAppBoundary, SemanticArmRoot, SemanticArmRoots,
    SemanticMemoryAppBoundary, SourceSnapshotVerifier, TaskSourceFence, TaskSplit,
    run_live_semantic_diagnostic, semantic_live_provider_templates,
    validate_live_semantic_diagnostic_config,
};
use mcp_vault_providers::{
    ProviderRuntimeSnapshot, StrictFunctionCallIssue, StructuredJsonDiagnostic,
    StructuredJsonFinishReason, StructuredJsonParseIssue, StructuredJsonParserCategory,
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn isolated_tempdir() -> TempDir {
    tempfile::Builder::new()
        .prefix("mcp-vault-diagnostic-")
        .tempdir_in("/private/tmp")
        .unwrap()
}

#[derive(Clone, Copy)]
enum ProviderMode {
    Success,
    Empty,
    ObservationError,
    ParseError,
    ProtocolError,
    CompositionError,
}

struct FakeProvider {
    mode: ProviderMode,
    calls: AtomicU32,
}

#[async_trait]
impl ProviderAppBoundary for FakeProvider {
    async fn generate(
        &self,
        request: LiveProviderRequest,
    ) -> Result<LiveProviderOutput, LiveProviderError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if matches!(self.mode, ProviderMode::ParseError) && call == 1 {
            return Err(LiveProviderError {
                code: "provider_structured_json_invalid".into(),
                schema_issue: None,
                schema_path: None,
                structured_json_diagnostic: Some(StructuredJsonDiagnostic {
                    parser_category: StructuredJsonParserCategory::Eof,
                    issue: StructuredJsonParseIssue::UnexpectedEof,
                    line: 3,
                    column: 11,
                    content_bytes: 17,
                    parsed_bytes: 17,
                    fence_detected: false,
                    finish_reason: Some(StructuredJsonFinishReason::Stop),
                }),
                protocol_issue: None,
            });
        }
        if matches!(self.mode, ProviderMode::ObservationError) && call == 1 {
            return Err(LiveProviderError {
                code: "provider_schema_invalid".into(),
                schema_issue: Some("enum_mismatch".into()),
                schema_path: Some("$.observations[0].body_block_indices[0]".into()),
                structured_json_diagnostic: Some(StructuredJsonDiagnostic {
                    parser_category: StructuredJsonParserCategory::Syntax,
                    issue: StructuredJsonParseIssue::GenericSyntax,
                    line: 1,
                    column: 7,
                    content_bytes: 9,
                    parsed_bytes: 9,
                    fence_detected: false,
                    finish_reason: Some(StructuredJsonFinishReason::Stop),
                }),
                protocol_issue: None,
            });
        }
        if matches!(self.mode, ProviderMode::CompositionError) && request.stage == "composition" {
            return Err(LiveProviderError {
                code: "provider_http_error".into(),
                ..Default::default()
            });
        }
        if matches!(self.mode, ProviderMode::ProtocolError) && call == 1 {
            return Err(LiveProviderError {
                code: "provider_response_invalid".into(),
                protocol_issue: Some(StrictFunctionCallIssue::NoToolCall),
                ..Default::default()
            });
        }
        let output = if request.stage == "observation" {
            let namespace = request.input["evidence_namespace"].as_str().unwrap();
            if matches!(self.mode, ProviderMode::Empty) {
                json!({"evidence_namespace":namespace,"outcome":"success_empty","observations":[]})
            } else {
                json!({"evidence_namespace":namespace,"outcome":"success_nonempty","observations":[{"kind":"decision","statement":"Keep the diagnostic bounded.","scope":"project","assertion_status":"source_asserted","admission_reason":"fixture","value_for_future_work":"bounded","body_block_indices":[1]}]})
            }
        } else {
            json!({"cards":[{"title":"Diagnostic card","observation_indices":[0]}]})
        };
        Ok(LiveProviderOutput {
            output,
            usage: None,
            cost_minor: None,
        })
    }
}

struct FakeSemantic {
    fence_failed: AtomicBool,
    prepared: AtomicUsize,
    accepted: AtomicUsize,
    submitted: AtomicUsize,
    aborted: AtomicUsize,
}

#[async_trait]
impl SemanticMemoryAppBoundary for FakeSemantic {
    async fn prepare(&self, _source: &EvalSource) -> Result<Value, String> {
        Ok(json!({"blocks":[{"local_id":"b1"}]}))
    }
    async fn build_pack(
        &self,
        _task: &EvalTask,
        _paths: &[String],
        _budget: &Budget,
    ) -> Result<Value, String> {
        Err("unused".into())
    }
    async fn submit_observation(
        &self,
        _source: &EvalSource,
        _proposal: &Value,
    ) -> Result<Value, String> {
        Err("unused".into())
    }
    async fn current_card_projection(&self, _source: &EvalSource) -> Result<Value, String> {
        Ok(json!({"card_count":1}))
    }
    async fn prepare_for_arm(
        &self,
        _arm: ComparisonArm,
        _source: &EvalSource,
    ) -> Result<Value, String> {
        self.prepared.fetch_add(1, Ordering::SeqCst);
        Ok(
            json!({"evidence_namespace":"h-diagnostic-fixture","blocks":[{"evidence_index":1,"text":"fixture","line_number":1,"kind":"paragraph"}]}),
        )
    }
    async fn accept_observation_for_arm(
        &self,
        _arm: ComparisonArm,
        _source: &EvalSource,
        proposal: &Value,
    ) -> Result<Value, String> {
        self.accepted.fetch_add(1, Ordering::SeqCst);
        if proposal["outcome"] == "success_empty" {
            Ok(json!({"status":"success_empty","observations":[],"groups":[]}))
        } else {
            Ok(
                json!({"status":"composition_required","observations":[{"statement":"x"}],"groups":[]}),
            )
        }
    }
    async fn submit_composition_for_arm(
        &self,
        _arm: ComparisonArm,
        _source: &EvalSource,
        _proposal: &Value,
    ) -> Result<Value, String> {
        self.submitted.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"state":"success_nonempty","card_count":1}))
    }
    async fn abort_semantic_for_arm(
        &self,
        _arm: ComparisonArm,
        _source: &EvalSource,
    ) -> Result<(), String> {
        self.aborted.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn verify_arm_task_sources(
        &self,
        _arm: ComparisonArm,
        _sources: &[EvalSource],
        _boundary: &str,
    ) -> Result<(), String> {
        if self.fence_failed.load(Ordering::SeqCst) {
            Err("source_mismatch".into())
        } else {
            Ok(())
        }
    }
}

struct FakeVerifier;
impl SourceSnapshotVerifier for FakeVerifier {
    fn snapshot(&self, source: &EvalSource) -> Result<AuthoritativeSourceSnapshot, EvalError> {
        Ok(AuthoritativeSourceSnapshot {
            logical_id: source.logical_id.clone(),
            vault_id: source.vault_id.clone(),
            file_id: source.file_id.clone(),
            path: source.path.clone(),
            file_revision: source.file_revision,
            source_revision_id: source.source_revision_id.clone(),
            content_hash: source.content_hash.clone(),
            content: Vec::new(),
            authorized: true,
            authorization_revision: source.authorization_revision,
            profile_id: source.profile_id.clone(),
            rules_revision: source.rules_revision,
            source_generation: source.source_generation,
            extraction_commit_sequence: source.extraction_commit_sequence,
        })
    }
}

fn config(root: &Path) -> LiveEvaluationConfig {
    let sources: Vec<_> = (0..30)
        .map(|i| EvalSource {
            logical_id: format!("S{i:02}"),
            synthetic_placeholder: false,
            vault_id: "vault".into(),
            file_id: format!("file-{i:02}"),
            path: format!("docs/adr/{i:02}.md"),
            file_revision: 1,
            content_hash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
            logical_block_count: 0,
            source_revision_id: format!("rev-{i:02}"),
            authorization_revision: 1,
            profile_id: "profile".into(),
            rules_revision: 1,
            source_generation: 1,
            extraction_commit_sequence: 1,
        })
        .collect();
    let tasks: Vec<_> = (0..60)
        .map(|i| {
            let source_id = format!("S{:02}", i % 30);
            EvalTask {
                id: format!("Q{i:02}"),
                split: TaskSplit::Holdout,
                query_refs: vec![format!("q{i:02}")],
                query: "diagnostic query".into(),
                source_ids: vec![source_id.clone()],
                source_fence: vec![TaskSourceFence {
                    source_id,
                    file_revision: 1,
                    source_revision_id: format!("rev-{:02}", i % 30),
                    content_hash:
                        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
                }],
                must_preserve: vec![],
                must_not_infer: vec![],
                expected_usable_information: vec![],
                expected_no_answer: false,
                expected_status: "supported".into(),
                expected_source_relations: Vec::<ExpectedRelation>::new(),
                severity: "high".into(),
            }
        })
        .collect();
    let ids: Vec<_> = sources.iter().map(|s| s.logical_id.clone()).collect();
    let task_ids: Vec<_> = tasks.iter().map(|t| t.id.clone()).collect();
    let budget = Budget {
        max_entries: 10,
        max_bytes: 1000,
        max_tokens: 4096,
        external_request_budget: 400,
    };
    let comparison = |arm| ComparisonConfig {
        arm,
        source_ids: ids.clone(),
        task_ids: task_ids.clone(),
        budget: budget.clone(),
        index_profile_id: "index-frozen-v1".into(),
        prompt_id: "semantic-cards-tracked-adr-m6-v12".into(),
        schema_id: "semantic-cards-m6-json-v8".into(),
        answer_model_id: "model".into(),
    };
    let artifact = root.join("artifacts");
    let runtime_snapshot: ProviderRuntimeSnapshot = serde_json::from_value(json!({
        "fingerprint":"fixture-fingerprint","provider_id":"provider","provider_type":"openai_compatible","endpoint":"http://127.0.0.1:1/v1","provider_revision":1,"provider_enabled":true,
        "settings":{"timeout_ms":30000,"stream_first_event_timeout_ms":30000,"stream_idle_timeout_ms":30000,"stream_total_timeout_ms":60000,"connect_timeout_ms":1000,"max_retries":0,"max_concurrency":1,"max_request_bytes":1000000,"max_response_bytes":1000000,"allow_private_networks":true,"header_names":[],"organization":null,"model_cache_configured":false},
        "model_id":"model","external_model_id":"model","model_revision":1,"model_enabled":true,"capabilities":{},"model_settings":{},"mode":"remote_allowed","mode_revision":1
    })).unwrap();
    LiveEvaluationConfig {
        schema_version: LIVE_RUN_SCHEMA.into(),
        semantic_protocol: "m1-two-stage-v2".into(),
        manifest: EvaluationManifest {
            schema_version: "semantic-memory-eval-manifest-v1".into(),
            dataset_id: "semantic-memory-synthetic-m6-v1".into(),
            synthetic_only: false,
            sources,
            tasks,
        },
        run_config: EvaluationRunConfig {
            schema_version: "semantic-memory-eval-run-config-v1".into(),
            mode: EvalMode::Live,
            allow_live: true,
            explicit_live_authorization: true,
            source_allowlist: ids
                .iter()
                .flat_map(|id| [id.clone(), format!("file-{}", &id[1..])])
                .collect(),
            cost_budget_minor: 1,
            artifact_root: Some(artifact.to_string_lossy().into()),
            comparisons: vec![
                comparison(ComparisonArm::A),
                comparison(ComparisonArm::B),
                comparison(ComparisonArm::C),
            ],
        },
        run_root: root.to_string_lossy().into(),
        source_root: root.join("source").to_string_lossy().into(),
        state_root: root.join("state").to_string_lossy().into(),
        history_root: root.join("history").to_string_lossy().into(),
        artifact_root: artifact.to_string_lossy().into(),
        isolated_vault_id: "vault".into(),
        semantic_arm_roots: SemanticArmRoots {
            b: SemanticArmRoot {
                vault_id: "b".into(),
                vault_slug: "b".into(),
                source_root: root.join("b-source").to_string_lossy().into(),
                state_root: root.join("b-state").to_string_lossy().into(),
                history_root: root.join("b-history").to_string_lossy().into(),
            },
            c: SemanticArmRoot {
                vault_id: "c".into(),
                vault_slug: "c".into(),
                source_root: root.join("c-source").to_string_lossy().into(),
                state_root: root.join("c-state").to_string_lossy().into(),
                history_root: root.join("c-history").to_string_lossy().into(),
            },
        },
        current_schema: LIVE_RUN_SCHEMA.into(),
        task_budget: 60,
        unbounded_cost_authorized: true,
        provider_model_id: Some("model".into()),
        provider_runtime_snapshot: Some(runtime_snapshot),
        provider_templates: semantic_live_provider_templates("model", 30),
        isolated_master_key_path: None,
    }
}

async fn run(
    root: &Path,
    mode: ProviderMode,
    fence_failed: bool,
) -> (
    mcp_vault_eval::LiveSemanticDiagnosticResult,
    Arc<FakeProvider>,
    Arc<FakeSemantic>,
) {
    let cfg = config(root);
    for path in [
        cfg.run_root.as_str(),
        cfg.source_root.as_str(),
        cfg.state_root.as_str(),
        cfg.history_root.as_str(),
        cfg.semantic_arm_roots.b.source_root.as_str(),
        cfg.semantic_arm_roots.b.state_root.as_str(),
        cfg.semantic_arm_roots.b.history_root.as_str(),
        cfg.semantic_arm_roots.c.source_root.as_str(),
        cfg.semantic_arm_roots.c.state_root.as_str(),
        cfg.semantic_arm_roots.c.history_root.as_str(),
        cfg.artifact_root.as_str(),
    ] {
        std::fs::create_dir_all(path).unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
    }
    let provider = Arc::new(FakeProvider {
        mode,
        calls: AtomicU32::new(0),
    });
    let semantic = Arc::new(FakeSemantic {
        fence_failed: AtomicBool::new(fence_failed),
        prepared: AtomicUsize::new(0),
        accepted: AtomicUsize::new(0),
        submitted: AtomicUsize::new(0),
        aborted: AtomicUsize::new(0),
    });
    let result = run_live_semantic_diagnostic(
        &cfg,
        &FakeVerifier,
        provider.as_ref(),
        semantic.as_ref(),
        DiagnosticSelection {
            arm: ComparisonArm::B,
            source_id: "S00".into(),
        },
    )
    .await
    .unwrap();
    (result, provider, semantic)
}

#[tokio::test]
async fn diagnostic_success_empty_and_nonempty_are_bounded() {
    let empty = isolated_tempdir();
    let (result, provider, semantic) = run(empty.path(), ProviderMode::Empty, false).await;
    assert_eq!(result.status, "diagnostic_success_empty");
    assert_eq!(result.provider_requests, 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(semantic.submitted.load(Ordering::SeqCst), 0);
    assert_eq!(result.quality_claim, "not_evaluated");
    assert_eq!(result.m6_acceptance, "not_run");
    let full = isolated_tempdir();
    let (result, provider, semantic) = run(full.path(), ProviderMode::Success, false).await;
    assert_eq!(result.status, "diagnostic_success_nonempty");
    assert_eq!(result.provider_requests, 2);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(semantic.submitted.load(Ordering::SeqCst), 1);
    assert_eq!(result.quality_claim, "not_evaluated");
}

#[tokio::test]
async fn diagnostic_provider_and_fence_failures_stop_without_retry() {
    let root = isolated_tempdir();
    let (result, provider, semantic) =
        run(root.path(), ProviderMode::ObservationError, false).await;
    assert_eq!(result.provider_requests, 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(semantic.aborted.load(Ordering::SeqCst), 1);
    let error = serde_json::to_value(&result).unwrap();
    assert_eq!(error["first_error"]["schema_issue"], "enum_mismatch");
    assert_eq!(
        error["first_error"]["structured_json_diagnostic"]["parser_category"],
        "syntax"
    );
    assert_eq!(
        error["first_error"]["structured_json_diagnostic"]["content_bytes"],
        9
    );
    let root = isolated_tempdir();
    let (result, provider, semantic) = run(root.path(), ProviderMode::ProtocolError, false).await;
    assert_eq!(result.provider_requests, 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(semantic.aborted.load(Ordering::SeqCst), 1);
    let error = serde_json::to_value(&result).unwrap();
    assert_eq!(error["first_error"]["protocol_issue"], "no_tool_call");
    assert_eq!(error["first_error"]["code"], "provider_response_invalid");
    assert!(error.to_string().contains("protocol_issue"));
    assert!(!error.to_string().contains("arguments"));

    let root = isolated_tempdir();
    let (result, provider, semantic) =
        run(root.path(), ProviderMode::CompositionError, false).await;
    assert_eq!(result.provider_requests, 2);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(semantic.aborted.load(Ordering::SeqCst), 1);
    assert_eq!(result.status, "diagnostic_failed");
    let root = isolated_tempdir();
    let (result, provider, semantic) = run(root.path(), ProviderMode::ParseError, false).await;
    assert_eq!(result.provider_requests, 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(semantic.aborted.load(Ordering::SeqCst), 1);
    let error = serde_json::to_value(&result).unwrap();
    assert_eq!(
        error["first_error"]["code"],
        "provider_structured_json_invalid"
    );
    assert!(error["first_error"]["schema_issue"].is_null());
    assert_eq!(
        error["first_error"]["structured_json_diagnostic"]["issue"],
        "unexpected_eof"
    );
    let root = isolated_tempdir();
    let (result, provider, semantic) = run(root.path(), ProviderMode::Success, true).await;
    assert_eq!(result.provider_requests, 0);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert_eq!(semantic.prepared.load(Ordering::SeqCst), 0);
    assert_eq!(result.status, "diagnostic_failed");
}

#[test]
fn diagnostic_selection_and_nonempty_artifact_root_fail_before_calls() {
    let root = isolated_tempdir();
    let mut cfg = config(root.path());
    let err = validate_live_semantic_diagnostic_config(
        &cfg,
        &DiagnosticSelection {
            arm: ComparisonArm::A,
            source_id: "S00".into(),
        },
    )
    .unwrap_err();
    assert!(matches!(err, EvalError::LiveConfig(_)));
    let marker = root.path().join("artifacts");
    std::fs::create_dir_all(&marker).unwrap();
    std::fs::write(marker.join("keep"), b"marker").unwrap();
    cfg.artifact_root = marker.to_string_lossy().into();
    let err = validate_live_semantic_diagnostic_config(
        &cfg,
        &DiagnosticSelection {
            arm: ComparisonArm::B,
            source_id: "S99".into(),
        },
    )
    .unwrap_err();
    assert!(matches!(err, EvalError::LiveConfig(_)));
    assert!(marker.join("keep").exists());
}
