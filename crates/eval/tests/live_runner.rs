use async_trait::async_trait;
use mcp_vault_eval::{
    AuthoritativeSourceSnapshot, Budget, ComparisonArm, ComparisonConfig, EvalMode, EvalSource,
    EvalTask, EvaluationManifest, EvaluationRunConfig, LIVE_RUN_SCHEMA, LiveEvaluationConfig,
    LiveProviderError, LiveProviderOutput, LiveProviderRequest, LiveRunStatus, ProviderAppBoundary,
    ProviderRuntimeSnapshot, ProviderStageTemplate, SemanticArmRoot, SemanticArmRoots,
    SemanticMemoryAppBoundary, SourceSnapshotVerifier, TaskSplit, prepare_live_private_directories,
    run_live_evaluation, semantic_a80_provider_templates, semantic_live_provider_templates,
    validate_external_model_identity, validate_isolated_master_key_path,
    validate_isolated_state_database_leaf, validate_live_evaluation_config,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Mutex;
use std::{
    path::Path,
    process::Command,
    sync::atomic::{AtomicU32, AtomicUsize, Ordering},
};

#[test]
fn live_cli_requires_the_exact_authorization_flag_without_opening_state() {
    let output = Command::new(env!("CARGO_BIN_EXE_semantic-card-live-eval"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--run-authorized-real-semantic-evaluation"));
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(unix)]
#[test]
fn isolated_state_database_leaf_rejects_external_symlinks_and_nonregular_files() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let run_root = temp.path().join("run");
    let state_root = run_root.join("state");
    std::fs::create_dir_all(&state_root).unwrap();
    let external_db = temp.path().join("external.sqlite3");
    std::fs::write(&external_db, b"not a real sqlite file").unwrap();
    symlink(&external_db, state_root.join("state.sqlite3")).unwrap();
    assert!(
        validate_isolated_state_database_leaf(run_root.to_str().unwrap(), &state_root,).is_err()
    );

    std::fs::remove_file(state_root.join("state.sqlite3")).unwrap();
    std::fs::create_dir(state_root.join("state.sqlite3")).unwrap();
    assert!(
        validate_isolated_state_database_leaf(run_root.to_str().unwrap(), &state_root,).is_err()
    );

    std::fs::remove_dir(state_root.join("state.sqlite3")).unwrap();
    std::fs::write(state_root.join("state.sqlite3"), b"local leaf").unwrap();
    assert_eq!(
        validate_isolated_state_database_leaf(run_root.to_str().unwrap(), &state_root).unwrap(),
        state_root.join("state.sqlite3")
    );

    std::fs::remove_file(state_root.join("state.sqlite3")).unwrap();
    std::fs::hard_link(&external_db, state_root.join("state.sqlite3")).unwrap();
    assert!(
        validate_isolated_state_database_leaf(run_root.to_str().unwrap(), &state_root).is_err(),
        "a canonical path inside run_root must not hide a shared inode"
    );
}

#[cfg(not(unix))]
#[test]
fn isolated_state_database_leaf_rejects_missing_leaf_before_creation_without_link_count() {
    let temp = tempfile::tempdir().unwrap();
    let run_root = temp.path().join("run");
    let state_root = run_root.join("state");
    let leaf = state_root.join("state.sqlite3");

    assert!(matches!(
        validate_isolated_state_database_leaf(run_root.to_str().unwrap(), &state_root),
        Err(mcp_vault_eval::EvalError::LiveConfig(
            "isolated SQLite leaf hard-link count cannot be verified on this platform"
        ))
    ));
    assert!(!leaf.exists(), "preflight must not create the SQLite leaf");
    assert!(!state_root.exists(), "preflight must not create state_root");
}

#[cfg(not(unix))]
#[test]
fn m6_live_preflight_fails_closed_before_creating_any_run_root() {
    let temp = tempfile::tempdir().unwrap();
    let (config, _) = config(temp.path());

    assert!(matches!(
        validate_live_evaluation_config(&config),
        Err(mcp_vault_eval::EvalError::LiveConfig(
            "private live-run filesystem permissions cannot be verified on this platform"
        ))
    ));
    assert!(!Path::new(&config.state_root).exists());
    assert!(!Path::new(&config.history_root).exists());
    assert!(!Path::new(&config.artifact_root).exists());
}

#[cfg(not(unix))]
#[tokio::test]
async fn live_runner_fails_closed_before_state_or_artifact_creation_without_link_count() {
    let temp = tempfile::tempdir().unwrap();
    let (config, content) = config(temp.path());
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };

    let error = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        mcp_vault_eval::EvalError::LiveConfig(
            "private live-run filesystem permissions cannot be verified on this platform"
        )
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert!(!Path::new(&config.state_root).exists());
    assert!(!Path::new(&config.artifact_root).exists());
}

#[cfg(unix)]
#[test]
fn live_preflight_rejects_hardlinked_baseline_and_semantic_arm_databases() {
    use std::os::unix::fs::PermissionsExt;

    for arm in ["A", "B", "C"] {
        let temp = tempfile::tempdir().unwrap();
        let (config, _) = config(temp.path());
        let state_root = match arm {
            "A" => Path::new(&config.state_root),
            "B" => Path::new(&config.semantic_arm_roots.b.state_root),
            "C" => Path::new(&config.semantic_arm_roots.c.state_root),
            _ => unreachable!(),
        };
        std::fs::create_dir_all(state_root).unwrap();
        std::fs::set_permissions(state_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let outside_db = temp.path().join(format!("outside-{arm}.sqlite3"));
        std::fs::write(&outside_db, b"shared state inode").unwrap();
        std::fs::hard_link(&outside_db, state_root.join("state.sqlite3")).unwrap();
        assert!(
            validate_live_evaluation_config(&config).is_err(),
            "{arm} state database hard link must fail live preflight"
        );
    }
}

fn config(root: &Path) -> (LiveEvaluationConfig, String) {
    let root = std::fs::canonicalize(root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let source_root = root.join("source");
    std::fs::create_dir_all(&source_root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&source_root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let content = b"# Decision\nKeep the rollback step.\n";
    let source = EvalSource {
        logical_id: "S01".into(),
        synthetic_placeholder: false,
        vault_id: "vault-real".into(),
        file_id: "file-real-1".into(),
        path: "notes/decision.md".into(),
        file_revision: 3,
        content_hash: hash(content),
        logical_block_count: 0,
        source_revision_id: "revision-real-1".into(),
        authorization_revision: 7,
        profile_id: "semantic-profile-v1".into(),
        rules_revision: 2,
        source_generation: 4,
        extraction_commit_sequence: 9,
    };
    let task = EvalTask {
        id: "Q01".into(),
        split: TaskSplit::Holdout,
        query_refs: vec!["S01".into()],
        query: "What rollback condition must be retained?".into(),
        source_ids: vec!["S01".into()],
        source_fence: vec![mcp_vault_eval::TaskSourceFence {
            source_id: "S01".into(),
            file_revision: 3,
            source_revision_id: "revision-real-1".into(),
            content_hash: hash(content),
        }],
        must_preserve: vec!["rollback step".into()],
        must_not_infer: vec!["deployment success".into()],
        expected_usable_information: vec!["rollback".into()],
        expected_no_answer: false,
        expected_status: "supported".into(),
        expected_source_relations: vec![],
        severity: "high".into(),
    };
    let manifest = EvaluationManifest {
        schema_version: "semantic-memory-eval-manifest-v1".into(),
        dataset_id: "live-contract".into(),
        synthetic_only: false,
        sources: vec![source],
        tasks: vec![task],
    };
    let budget = Budget {
        max_entries: 12,
        max_bytes: 64_000,
        max_tokens: 4_096,
        external_request_budget: 16,
    };
    let arm = |arm| ComparisonConfig {
        arm,
        source_ids: vec!["S01".into()],
        task_ids: vec!["Q01".into()],
        budget: budget.clone(),
        index_profile_id: "index-frozen-v1".into(),
        prompt_id: "prompt-frozen-v1".into(),
        schema_id: "schema-frozen-v1".into(),
        answer_model_id: "answer-model-v1".into(),
    };
    let artifact_root = root.join("artifacts");
    let run = EvaluationRunConfig {
        schema_version: "semantic-memory-eval-run-config-v1".into(),
        mode: EvalMode::Live,
        allow_live: true,
        explicit_live_authorization: true,
        source_allowlist: vec!["S01".into(), "file-real-1".into()],
        cost_budget_minor: 0,
        artifact_root: Some(artifact_root.display().to_string()),
        comparisons: vec![
            arm(ComparisonArm::A),
            arm(ComparisonArm::B),
            arm(ComparisonArm::C),
        ],
    };
    (
        LiveEvaluationConfig {
            schema_version: LIVE_RUN_SCHEMA.into(),
            semantic_protocol: String::new(),
            manifest,
            run_config: run,
            run_root: root.display().to_string(),
            source_root: source_root.display().to_string(),
            state_root: root.join("state").display().to_string(),
            history_root: root.join("history").display().to_string(),
            artifact_root: artifact_root.display().to_string(),
            isolated_vault_id: "live-isolated-vault".into(),
            semantic_arm_roots: SemanticArmRoots {
                b: SemanticArmRoot {
                    vault_slug: "live-b".into(),
                    vault_id: "live-b-vault".into(),
                    source_root: root.join("arm-b/source").display().to_string(),
                    state_root: root.join("arm-b/state").display().to_string(),
                    history_root: root.join("arm-b/history").display().to_string(),
                },
                c: SemanticArmRoot {
                    vault_slug: "live-c".into(),
                    vault_id: "live-c-vault".into(),
                    source_root: root.join("arm-c/source").display().to_string(),
                    state_root: root.join("arm-c/state").display().to_string(),
                    history_root: root.join("arm-c/history").display().to_string(),
                },
            },
            current_schema: LIVE_RUN_SCHEMA.into(),
            task_budget: 1,
            unbounded_cost_authorized: true,
            provider_model_id: Some("internal-model-v1".into()),
            provider_runtime_snapshot: Some(ProviderRuntimeSnapshot {
                fingerprint: "frozen-runtime-fingerprint".into(),
                provider_id: "provider-1".into(),
                provider_type: "openai_compatible".into(),
                endpoint: "https://provider.invalid/v1/".into(),
                provider_revision: 1,
                provider_enabled: true,
                settings: mcp_vault_providers::SafeProviderSettings {
                    timeout_ms: 30_000,
                    stream_first_event_timeout_ms: 120_000,
                    stream_idle_timeout_ms: 120_000,
                    stream_total_timeout_ms: 600_000,
                    connect_timeout_ms: 5_000,
                    max_retries: 0,
                    max_concurrency: 1,
                    max_request_bytes: 1_048_576,
                    max_response_bytes: 1_048_576,
                    allow_private_networks: false,
                    header_names: vec![],
                    organization: None,
                    model_cache_configured: false,
                },
                model_id: "internal-model-v1".into(),
                external_model_id: "answer-model-v1".into(),
                model_revision: 1,
                model_enabled: true,
                capabilities: mcp_vault_providers::ModelCapabilities::default(),
                model_settings: mcp_vault_providers::ModelSettings::default(),
                mode: mcp_vault_providers::ProviderMode::Enabled,
                mode_revision: Some(1),
            }),
            provider_templates: ["observation", "relation", "answer"]
                .into_iter()
                .map(|stage| ProviderStageTemplate {
                    stage: stage.into(),
                    model_id: "answer-model-v1".into(),
                    prompt_id: "prompt-frozen-v1".into(),
                    schema_id: "schema-frozen-v1".into(),
                    index_profile_id: "index-frozen-v1".into(),
                    system: "frozen system".into(),
                    schema_name: "semantic-eval".into(),
                    schema: json!({"type":"object"}),
                    max_output_tokens: 64,
                    temperature: Some(0.0),
                    timeout_seconds: 30,
                })
                .collect(),
            isolated_master_key_path: None,
        },
        String::from_utf8(content.to_vec()).unwrap(),
    )
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_rejects_configuration_drift_after_preparation_before_network() {
    use mcp_vault_auth::{AuthService, MasterKeyRing};
    use mcp_vault_domain::{Revision, VaultContext, VaultId, VaultSlug};
    use mcp_vault_eval::{LiveCliConfig, build_live_runtime, write_live_preparation_seal};
    use mcp_vault_providers::{
        ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderMode,
        ProviderService, ProviderSettings,
    };
    use mcp_vault_state::{StateStore, VaultStatus};
    use std::os::unix::fs::PermissionsExt;

    for drift in ["token_limit", "capabilities", "endpoint"] {
        let temp = tempfile::tempdir().unwrap();
        let (mut cfg, _) = config(temp.path());
        let key_path = temp.path().join("secrets/master.key");
        prepare_live_private_directories(&cfg, &key_path).unwrap();
        std::fs::write(&key_path, [7_u8; 32]).unwrap();
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let db = Path::new(&cfg.state_root).join("state.sqlite3");
        let state = StateStore::connect_and_migrate(&format!("sqlite://{}", db.display()))
            .await
            .unwrap();
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o600)).unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("live-a").unwrap(),
            cfg.source_root.clone().into(),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "fixture", VaultStatus::Active)
            .await
            .unwrap();
        let service = ProviderService::new(
            state.clone(),
            AuthService::new(
                state.auth(),
                MasterKeyRing::from_bytes(1, &[7; 32]).unwrap(),
            ),
        );
        service
            .set_provider_mode(&context, ProviderMode::Enabled, None)
            .await
            .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut provider = service
            .create_provider(ProviderInput {
                name: "sealed-runtime-fixture".into(),
                kind: ProviderKind::OpenAiCompatible,
                base_url: url::Url::parse(&format!(
                    "http://{}/v1/",
                    listener.local_addr().unwrap()
                ))
                .unwrap(),
                settings: ProviderSettings {
                    max_retries: 0,
                    ..Default::default()
                },
                enabled: true,
                secret: None,
            })
            .await
            .unwrap();
        let mut model = service
            .register_model(ModelInput {
                provider_id: provider.id,
                external_model_id: "answer-model-v1".into(),
                capabilities: ModelCapabilities {
                    structured_output: true,
                    ..Default::default()
                },
                settings: ModelSettings {
                    generation_token_limit: Some(32_768),
                    ..Default::default()
                },
                enabled: true,
            })
            .await
            .unwrap();
        cfg.isolated_vault_id = context.id().to_string();
        cfg.manifest.sources[0].vault_id = cfg.isolated_vault_id.clone();
        cfg.provider_model_id = Some(model.id.to_string());
        cfg.provider_runtime_snapshot =
            Some(service.runtime_snapshot(&context, model.id).await.unwrap());
        cfg.isolated_master_key_path = Some(key_path.display().to_string());
        validate_live_evaluation_config(&cfg).unwrap();
        write_live_preparation_seal(&cfg, &key_path).unwrap();

        match drift {
            "token_limit" => {
                model.settings["generation_token_limit"] = json!(131_072);
                state.providers().update_model(&model).await.unwrap();
            }
            "capabilities" => {
                model.capabilities["structured_output"] = json!(false);
                state.providers().update_model(&model).await.unwrap();
            }
            "endpoint" => {
                provider.base_url.push_str("changed/");
                state.providers().update_provider(&provider).await.unwrap();
            }
            _ => unreachable!(),
        }
        let cli = LiveCliConfig {
            model_id: model.id.to_string(),
            templates: cfg.provider_templates.clone(),
            evaluation: cfg,
            vault_slug: "live-a".into(),
            master_key_path: key_path.display().to_string(),
        };
        let error = match build_live_runtime(cli, None).await {
            Ok(_) => panic!("{drift} must invalidate the sealed preparation"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "Provider runtime differs from the sealed preparation",
            "{drift}"
        );
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "{drift} must be rejected before connecting"
        );
    }
}

#[cfg(unix)]
#[test]
fn live_preparation_creates_private_roots_and_rejects_broad_modes_or_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root_names = [
        "run",
        "source",
        "state",
        "history",
        "artifact",
        "b_source",
        "b_state",
        "b_history",
        "c_source",
        "c_state",
        "c_history",
        "key_parent",
    ];
    for name in root_names {
        let temp = tempfile::tempdir().unwrap();
        let (mut config, _) = config(temp.path());
        let key_path = Path::new(&config.run_root).join("keys/master-key");
        config.isolated_master_key_path = Some(key_path.display().to_string());
        prepare_live_private_directories(&config, &key_path).unwrap();
        let root = match name {
            "run" => Path::new(&config.run_root),
            "source" => Path::new(&config.source_root),
            "state" => Path::new(&config.state_root),
            "history" => Path::new(&config.history_root),
            "artifact" => Path::new(&config.artifact_root),
            "b_source" => Path::new(&config.semantic_arm_roots.b.source_root),
            "b_state" => Path::new(&config.semantic_arm_roots.b.state_root),
            "b_history" => Path::new(&config.semantic_arm_roots.b.history_root),
            "c_source" => Path::new(&config.semantic_arm_roots.c.source_root),
            "c_state" => Path::new(&config.semantic_arm_roots.c.state_root),
            "c_history" => Path::new(&config.semantic_arm_roots.c.history_root),
            "key_parent" => key_path.parent().unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(
            std::fs::metadata(root).unwrap().permissions().mode() & 0o777,
            0o700,
            "new {name} directory must be explicitly private"
        );
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(
            validate_live_evaluation_config(&config).is_err(),
            "existing {name} directory with group permissions must fail preflight"
        );
    }

    let temp = tempfile::tempdir().unwrap();
    let (config, _) = config(temp.path());
    let key_path = Path::new(&config.run_root).join("keys/master-key");
    prepare_live_private_directories(&config, &key_path).unwrap();
    let outside = temp.path().join("outside-state");
    std::fs::create_dir(&outside).unwrap();
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::remove_dir(&config.state_root).unwrap();
    symlink(outside, &config.state_root).unwrap();
    assert!(validate_live_evaluation_config(&config).is_err());
}

#[cfg(unix)]
#[test]
fn isolated_master_key_must_be_private_mode_0600() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let (config, _) = config(temp.path());
    let key_path = Path::new(&config.run_root).join("keys/master-key");
    prepare_live_private_directories(&config, &key_path).unwrap();
    std::fs::write(&key_path, b"synthetic-test-key-placeholder").unwrap();
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(validate_isolated_master_key_path(&config.run_root, &key_path).is_err());
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    validate_isolated_master_key_path(&config.run_root, &key_path).unwrap();
}

fn extra_source(logical_id: &str, file_id: &str, path: &str, body: &[u8]) -> EvalSource {
    EvalSource {
        logical_id: logical_id.into(),
        synthetic_placeholder: false,
        vault_id: "vault-real".into(),
        file_id: file_id.into(),
        path: path.into(),
        file_revision: 1,
        content_hash: hash(body),
        logical_block_count: 0,
        source_revision_id: format!("revision-{logical_id}"),
        authorization_revision: 7,
        profile_id: "semantic-profile-v1".into(),
        rules_revision: 2,
        source_generation: 1,
        extraction_commit_sequence: 1,
    }
}

struct Verifier {
    content: String,
}
impl SourceSnapshotVerifier for Verifier {
    fn snapshot(
        &self,
        source: &EvalSource,
    ) -> Result<AuthoritativeSourceSnapshot, mcp_vault_eval::EvalError> {
        Ok(AuthoritativeSourceSnapshot {
            logical_id: source.logical_id.clone(),
            vault_id: source.vault_id.clone(),
            file_id: source.file_id.clone(),
            path: source.path.clone(),
            file_revision: source.file_revision,
            source_revision_id: source.source_revision_id.clone(),
            content_hash: source.content_hash.clone(),
            content: self.content.as_bytes().to_vec(),
            authorized: true,
            authorization_revision: source.authorization_revision,
            profile_id: source.profile_id.clone(),
            rules_revision: source.rules_revision,
            source_generation: source.source_generation,
            extraction_commit_sequence: source.extraction_commit_sequence,
        })
    }
}

struct DriftingVerifier {
    inner: Verifier,
    drift_after: usize,
    calls: AtomicUsize,
}

impl SourceSnapshotVerifier for DriftingVerifier {
    fn snapshot(
        &self,
        source: &EvalSource,
    ) -> Result<AuthoritativeSourceSnapshot, mcp_vault_eval::EvalError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut snapshot = self.inner.snapshot(source)?;
        if call >= self.drift_after {
            snapshot.content = b"drifted after boundary".to_vec();
        }
        Ok(snapshot)
    }
}

struct FakeSemantic {
    prepare_count: AtomicUsize,
    submit_count: AtomicUsize,
}
const FAKE_ARM_COPY_DRIFT_AFTER_PACK: usize = usize::MAX;

impl FakeSemantic {
    fn drift_after_pack(&self) {
        self.submit_count
            .store(FAKE_ARM_COPY_DRIFT_AFTER_PACK, Ordering::SeqCst);
    }
}

#[async_trait]
impl SemanticMemoryAppBoundary for FakeSemantic {
    async fn ordinary_retrieve(
        &self,
        task: &EvalTask,
        sources: &[EvalSource],
        _budget: &Budget,
    ) -> Result<Value, String> {
        let mut source_ids = sources
            .iter()
            .map(|source| source.logical_id.clone())
            .collect::<Vec<_>>();
        source_ids.sort();
        let source_count = u32::try_from(source_ids.len()).unwrap_or(u32::MAX);
        let eligible_count = if task.query.contains("force-empty-index-results") {
            0
        } else {
            source_count
        };
        Ok(json!({
            "retrieval_strategy": "ordinary_note_v1",
            "task_id": task.id,
            "source_ids": sources.iter().map(|source| source.logical_id.clone()).collect::<Vec<_>>(),
            "coverage": {
                "expected_source_ids": source_ids.clone(),
                "current_indexed_source_ids": source_ids,
                "expected_source_count": source_count,
                "current_indexed_source_count": source_count,
                "coverage_ratio": 1.0,
                "complete": true,
                "candidate_count": eligible_count,
                "eligible_count": eligible_count,
                "available_result_count": eligible_count,
                "returned_hit_count": eligible_count,
                "stale_hit_count": 0,
            },
            "degradation_reasons": [],
        }))
    }

    async fn prepare(&self, _source: &EvalSource) -> Result<Value, String> {
        self.prepare_count.fetch_add(1, Ordering::SeqCst);
        Ok(
            json!({"evidence_namespace":"h-live-fixture","blocks":[{"evidence_index":1,"text":"redacted at artifact boundary","line_number":1,"kind":"paragraph"}]}),
        )
    }
    async fn submit_observation(
        &self,
        _source: &EvalSource,
        proposal: &Value,
    ) -> Result<Value, String> {
        if self.submit_count.load(Ordering::SeqCst) != FAKE_ARM_COPY_DRIFT_AFTER_PACK {
            self.submit_count.fetch_add(1, Ordering::SeqCst);
        }
        Ok(json!({"card_id":"card-1","statement":proposal["statement"]}))
    }

    async fn accept_observation_for_arm(
        &self,
        _arm: ComparisonArm,
        _source: &EvalSource,
        _proposal: &Value,
    ) -> Result<Value, String> {
        self.submit_count.fetch_add(1, Ordering::SeqCst);
        Ok(json!({
            "status":"composition_required",
            "groups":[{"group_id":"group-0001","allowed_observation_indices":[0]}],
            "observations":[{"statement":"retain rollback","body_block_ids":["b1"]}]
        }))
    }

    async fn fail_a80_batch_for_arm(
        &self,
        _arm: ComparisonArm,
        _source: &EvalSource,
        _batch_index: u32,
        _safe_error_code: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn reserve_a80_regen_for_arm(
        &self,
        _arm: ComparisonArm,
        _source: &EvalSource,
        _batch_index: u32,
    ) -> Result<String, String> {
        Ok("dispatching".into())
    }

    async fn submit_composition_for_arm(
        &self,
        _arm: ComparisonArm,
        _source: &EvalSource,
        _proposal: &Value,
    ) -> Result<Value, String> {
        self.submit_count.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"state":"success_nonempty","observation_count":1,"card_count":2}))
    }

    async fn abort_semantic_for_arm(
        &self,
        _arm: ComparisonArm,
        _source: &EvalSource,
    ) -> Result<(), String> {
        self.submit_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn verify_arm_task_sources(
        &self,
        _arm: ComparisonArm,
        _sources: &[EvalSource],
        boundary: &str,
    ) -> Result<(), String> {
        if boundary == "after_pack"
            && self.submit_count.load(Ordering::SeqCst) == FAKE_ARM_COPY_DRIFT_AFTER_PACK
        {
            Err("semantic_source_fence_mismatch".into())
        } else {
            Ok(())
        }
    }

    async fn current_card_projection(&self, _source: &EvalSource) -> Result<Value, String> {
        Ok(
            json!({"card_id":"card-1","assertions":[{"statement":"retain rollback","support_refs":["E1"],"conditions":["rollback"]}],"qualifiers":["rollback"],"evidence_refs":["E1"]}),
        )
    }

    async fn build_pack(
        &self,
        _task: &EvalTask,
        source_paths: &[String],
        _budget: &Budget,
    ) -> Result<Value, String> {
        Ok(json!({
            "current_context": [{
                "id":"card-rollback",
                "origin":"m1",
                "title":"Rollback decision",
                "kind":"decision",
                "scope_ref":"project",
                "assertion_status":"source_asserted",
                "temporal_scope":{},
                "core_assertions":["rollback"],
                "conditions":["rollback approved"],
                "exceptions":[],
                "ordered_steps":["preserve rollback step"],
                "unresolved_items":[],
                "optional_details":[],
                "source_references":source_paths.iter().map(|path| json!({
                    "source_id":"00000000-0000-0000-0000-000000000001",
                    "source_revision_id":"00000000-0000-0000-0000-000000000002",
                    "source_path":path,
                })).collect::<Vec<_>>(),
                "evidence_refs":["evidence-1"],
                "note_body":"must never reach Provider"
            }],
            "relevant_experiences":[],
            "conflicts_or_checks":[],
            "evidence_gaps":[],
            "related_sources":[],
            "diagnostics":[],
            "estimated_tokens":32
        }))
    }
}

#[tokio::test]
async fn two_stage_protocol_dispatches_observation_then_composition() {
    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    let mut value = serde_json::to_value(&config).unwrap();
    value["semantic_protocol"] = json!("m1-two-stage-v2");
    value["provider_templates"].as_array_mut().unwrap().push(json!({
        "stage":"composition","model_id":"answer-model-v1","prompt_id":"prompt-frozen-v1",
        "schema_id":"schema-frozen-v1","index_profile_id":"index-frozen-v1","system":"frozen system",
        "schema_name":"semantic-eval-composition","schema":{"type":"object"},"max_output_tokens":64,
        "temperature":0.0,"timeout_seconds":30
    }));
    config = serde_json::from_value(value).unwrap();
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();
    assert_eq!(result.status, LiveRunStatus::Completed);
    assert!(result.provider_requests > 0);
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        result.provider_requests
    );
    assert!(semantic.submit_count.load(Ordering::SeqCst) >= 4);
    let usage: Value =
        serde_json::from_slice(&std::fs::read(temp.path().join("artifacts/usage.json")).unwrap())
            .unwrap();
    assert_eq!(usage["latency"]["status"], "known");
    for arm in ["B", "C"] {
        for stage in ["observation", "composition", "answer"] {
            assert_eq!(
                usage["by_arm"][arm]["by_stage_latency"][stage]["generation_calls"],
                1
            );
        }
    }
    assert_eq!(
        usage["by_arm"]["C"]["by_stage_latency"]["relation"]["generation_calls"],
        1
    );
    let report = std::fs::read_to_string(temp.path().join("artifacts/report.md")).unwrap();
    assert!(report.contains("independent_agent_blind_review"));
    assert!(report.contains("\"human_review\": false"));
    assert!(report.contains("\"m6_acceptance\": \"not_evaluated\""));
    assert!(!report.contains("pending_manual_review"));
    let compositions =
        std::fs::read_to_string(temp.path().join("artifacts/observations.jsonl")).unwrap();
    assert!(compositions.contains("composition"));
}

#[tokio::test]
async fn a80_relation_provider_failure_isolated_and_c_answers_still_run() {
    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    config.semantic_protocol = "m1-a80-v1".into();
    config.manifest.sources[0].logical_block_count = 2;
    config.provider_templates = semantic_a80_provider_templates("answer-model-v1", 30);
    for comparison in &mut config.run_config.comparisons {
        comparison.prompt_id = "semantic-cards-tracked-adr-m6-v14".into();
        comparison.schema_id = "semantic-cards-m6-json-v10".into();
    }
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: Some(3),
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();
    assert_eq!(result.status, LiveRunStatus::Failed);
    assert_eq!(
        result.first_error_code.as_deref(),
        Some("evaluation_items_failed")
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 6);
    assert_eq!(result.completed_tasks, 1);
    let artifacts = Path::new(&config.artifact_root);
    let answers = std::fs::read_to_string(artifacts.join("answers.jsonl")).unwrap();
    assert!(answers.contains("\"task_id\":\"Q01\""));
    let failures = std::fs::read_to_string(artifacts.join("item-failures.jsonl")).unwrap();
    assert!(failures.contains("\"stage\":\"relation\""));
    assert!(failures.contains("local_fake_timeout"));
}

#[tokio::test]
async fn a80_shared_request_budget_failure_stops_before_later_tasks() {
    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    config.semantic_protocol = "m1-a80-v1".into();
    config.manifest.sources[0].logical_block_count = 2;
    let mut second_task = config.manifest.tasks[0].clone();
    second_task.id = "Q02".into();
    second_task.query_refs = vec!["Q02".into()];
    config.manifest.tasks.push(second_task);
    config.task_budget = 2;
    for comparison in &mut config.run_config.comparisons {
        comparison.task_ids = vec!["Q01".into(), "Q02".into()];
    }
    config.provider_templates = semantic_a80_provider_templates("answer-model-v1", 30);
    for comparison in &mut config.run_config.comparisons {
        comparison.prompt_id = "semantic-cards-tracked-adr-m6-v14".into();
        comparison.schema_id = "semantic-cards-m6-json-v10".into();
    }
    let provider = GlobalErrorProvider {
        calls: AtomicU32::new(0),
        fail_at: 3,
        code: "request_budget_exhausted",
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    assert_eq!(result.status, LiveRunStatus::Failed);
    assert_eq!(
        result.first_error_code.as_deref(),
        Some("request_budget_exhausted")
    );
    assert_eq!(result.completed_tasks, 0);
    let artifacts = Path::new(&config.artifact_root);
    let answers = std::fs::read_to_string(artifacts.join("answers.jsonl")).unwrap();
    assert!(!answers.contains("Q01"));
    assert!(!answers.contains("Q02"));
}

#[tokio::test]
async fn a80_answer_failure_stays_in_denominator_and_later_tasks_continue() {
    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    config.semantic_protocol = "m1-a80-v1".into();
    config.manifest.sources[0].logical_block_count = 2;
    let mut second_task = config.manifest.tasks[0].clone();
    second_task.id = "Q02".into();
    second_task.query_refs = vec!["Q02".into()];
    config.manifest.tasks.push(second_task);
    config.task_budget = 2;
    config.provider_templates = semantic_a80_provider_templates("answer-model-v1", 30);
    for comparison in &mut config.run_config.comparisons {
        comparison.prompt_id = "semantic-cards-tracked-adr-m6-v14".into();
        comparison.schema_id = "semantic-cards-m6-json-v10".into();
        comparison.task_ids.push("Q02".into());
    }
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        // B observation, C observation, relation, then A's first answer.
        fail_at: Some(4),
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };

    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();

    assert_eq!(provider.calls.load(Ordering::SeqCst), 10);
    assert_eq!(result.status, LiveRunStatus::Failed);
    assert_eq!(
        result.first_error_code.as_deref(),
        Some("evaluation_items_failed")
    );
    assert_eq!(result.completed_tasks, 2);
    let usage: Value = serde_json::from_slice(
        &std::fs::read(Path::new(&config.artifact_root).join("usage.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(usage["latency"]["status"], "known");
    assert_eq!(usage["latency"]["measured"]["transport_attempts"], 10);
    assert_eq!(
        usage["by_arm"]["A"]["by_stage_latency"]["answer"]["generation_calls"], 2,
        "failed answers must retain their measured call and original denominator"
    );
    let answers: Vec<Value> =
        std::fs::read_to_string(Path::new(&config.artifact_root).join("answers.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    assert!(answers.iter().any(|answer| {
        answer["task_id"] == "Q01" && answer["arm"] == "a" && answer["stage"] == "answer_failure"
    }));
    assert!(answers.iter().any(|answer| {
        answer["task_id"] == "Q02" && answer["arm"] == "c" && answer["stage"] == "answer"
    }));
    let report =
        std::fs::read_to_string(Path::new(&config.artifact_root).join("report.md")).unwrap();
    assert!(report.contains("not_evaluated"));
}

#[test]
fn a80_preflight_rejects_budget_above_the_full_run_limit() {
    let temp = tempfile::tempdir().unwrap();
    let (mut cfg, _) = config(temp.path());
    cfg.semantic_protocol = "m1-a80-v1".into();
    cfg.manifest.sources[0].logical_block_count = 2;
    cfg.provider_templates = semantic_a80_provider_templates("answer-model-v1", 30);
    for arm in &mut cfg.run_config.comparisons {
        arm.prompt_id = "semantic-cards-tracked-adr-m6-v14".into();
        arm.schema_id = "semantic-cards-m6-json-v10".into();
        arm.budget.external_request_budget = 161;
    }
    assert_eq!(
        validate_live_evaluation_config(&cfg).unwrap_err(),
        mcp_vault_eval::EvalError::LiveBlocked(
            "A80 request budget exceeds the 160-request run limit"
        )
    );
    assert!(!Path::new(&cfg.artifact_root).exists());
}

#[tokio::test]
async fn a80_auth_failure_stops_the_run_after_one_provider_call() {
    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    config.semantic_protocol = "m1-a80-v1".into();
    config.manifest.sources[0].logical_block_count = 2;
    config.provider_templates = semantic_a80_provider_templates("answer-model-v1", 30);
    for comparison in &mut config.run_config.comparisons {
        comparison.prompt_id = "semantic-cards-tracked-adr-m6-v14".into();
        comparison.schema_id = "semantic-cards-m6-json-v10".into();
    }
    let provider = AuthFailureProvider {
        calls: AtomicU32::new(0),
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(result.status, LiveRunStatus::Failed);
    assert_eq!(
        result.first_error_code.as_deref(),
        Some("provider_auth_failed")
    );
}

#[tokio::test]
async fn a80_recovers_one_local_schema_failure_and_records_each_budgeted_request() {
    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    config.semantic_protocol = "m1-a80-v1".into();
    config.manifest.sources[0].logical_block_count = 2;
    config.provider_templates = semantic_a80_provider_templates("answer-model-v1", 30);
    for comparison in &mut config.run_config.comparisons {
        comparison.prompt_id = "semantic-cards-tracked-adr-m6-v14".into();
        comparison.schema_id = "semantic-cards-m6-json-v10".into();
    }
    let provider = OneSchemaFailureProvider {
        calls: AtomicU32::new(0),
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };

    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();

    assert_eq!(provider.calls.load(Ordering::SeqCst), 7);
    assert_eq!(result.provider_requests, 7);
    assert_eq!(result.completed_tasks, 1);
    let attempts =
        std::fs::read_to_string(Path::new(&config.artifact_root).join("attempts.jsonl")).unwrap();
    let observation_attempts = attempts
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|attempt| attempt["stage"] == "observation")
        .count();
    assert_eq!(observation_attempts, 3);
    assert!(attempts.contains("\"source_id\":\"S01\""));
    assert_eq!(result.status, LiveRunStatus::Completed);
}

#[cfg(unix)]
#[tokio::test]
async fn a80_resume_reuses_card_and_consumes_existing_attempt_budget() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    config.semantic_protocol = "m1-a80-v1".into();
    config.manifest.sources[0].logical_block_count = 2;
    config.provider_templates = semantic_a80_provider_templates("answer-model-v1", 30);
    for comparison in &mut config.run_config.comparisons {
        comparison.prompt_id = "semantic-cards-tracked-adr-m6-v14".into();
        comparison.schema_id = "semantic-cards-m6-json-v10".into();
    }
    let key = Path::new(&config.run_root).join("keys/master-key");
    config.isolated_master_key_path = Some(key.display().to_string());
    prepare_live_private_directories(&config, &key).unwrap();
    let (manifest_hash, run_config_hash) = validate_live_evaluation_config(&config).unwrap();
    let live_config_hash = mcp_vault_eval::canonical_live_evaluation_hash(&config).unwrap();
    let artifacts = Path::new(&config.artifact_root);
    let checkpoint = json!({
        "schema_version":LIVE_RUN_SCHEMA,
        "manifest_hash":manifest_hash,
        "run_config_hash":run_config_hash,
        "live_config_hash":live_config_hash,
        "last_boundary":"observation_batch",
        "provider_requests":0,
        "completed_tasks":0,
        "status":"running",
        "first_error":null
    });
    let write_private = |name: &str, bytes: &[u8]| {
        let path = artifacts.join(name);
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    };
    write_private(
        "checkpoint.json",
        serde_json::to_vec(&checkpoint).unwrap().as_slice(),
    );
    write_private(
        "attempts.jsonl",
        format!(
            "{}\n",
            json!({"sequence":1,"stage":"observation","arm":"b","source_id":"S01","task_id":null,"batch_index":0,"status":"reserved"})
        )
        .as_bytes(),
    );
    write_private(
        "cards.jsonl",
        format!(
            "{}\n",
            json!({"sequence":1,"stage":"card","arm":"b","source_id":"S01","task_id":null,"pack_hash":null,"output":{"card_count":1,"cards":[]},"usage":null})
        )
        .as_bytes(),
    );
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let outcome = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic).await;
    let result = outcome.unwrap();
    assert_eq!(result.status, LiveRunStatus::Completed);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 5);
    assert_eq!(result.provider_requests, 6);
    assert_eq!(result.completed_tasks, 1);
    assert_eq!(semantic.prepare_count.load(Ordering::SeqCst), 1);
    let usage: Value =
        serde_json::from_slice(&std::fs::read(artifacts.join("usage.json")).unwrap()).unwrap();
    assert_eq!(usage["latency"]["status"], "partial");
    assert_eq!(usage["latency"]["measured"]["transport_attempts"], 5);
    assert_eq!(usage["latency"]["reserved_requests"], 6);
}

#[cfg(unix)]
#[tokio::test]
async fn a80_reserved_attempt_with_ready_batch_is_charged_and_never_replayed() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    config.semantic_protocol = "m1-a80-v1".into();
    config.manifest.sources[0].logical_block_count = 2;
    config.provider_templates = semantic_a80_provider_templates("answer-model-v1", 30);
    for comparison in &mut config.run_config.comparisons {
        comparison.prompt_id = "semantic-cards-tracked-adr-m6-v14".into();
        comparison.schema_id = "semantic-cards-m6-json-v10".into();
    }
    let key = Path::new(&config.run_root).join("keys/master-key");
    config.isolated_master_key_path = Some(key.display().to_string());
    prepare_live_private_directories(&config, &key).unwrap();
    let (manifest_hash, run_config_hash) = validate_live_evaluation_config(&config).unwrap();
    let live_config_hash = mcp_vault_eval::canonical_live_evaluation_hash(&config).unwrap();
    let artifacts = Path::new(&config.artifact_root);
    let write_private = |name: &str, bytes: &[u8]| {
        let path = artifacts.join(name);
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    };
    write_private(
        "checkpoint.json",
        serde_json::to_vec(&json!({
            "schema_version":LIVE_RUN_SCHEMA,
            "manifest_hash":manifest_hash,
            "run_config_hash":run_config_hash,
            "live_config_hash":live_config_hash,
            "last_boundary":"observation_batch",
            "provider_requests":0,
            "completed_tasks":0,
            "status":"running",
            "first_error":null
        }))
        .unwrap()
        .as_slice(),
    );
    write_private(
        "attempts.jsonl",
        format!(
            "{}\n",
            json!({"sequence":1,"stage":"observation","arm":"b","source_id":"S01","task_id":null,"batch_index":0,"status":"reserved"})
        )
        .as_bytes(),
    );
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };

    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();

    // C observation, relation, and three answers are dispatched. The reserved
    // B observation consumes the shared budget but is not replayed.
    assert_eq!(provider.calls.load(Ordering::SeqCst), 5);
    assert_eq!(result.provider_requests, 6);
    assert_eq!(result.status, LiveRunStatus::Failed);
    let attempts = std::fs::read_to_string(artifacts.join("attempts.jsonl")).unwrap();
    let b_observations = attempts
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|attempt| attempt["stage"] == "observation" && attempt["arm"] == "b")
        .count();
    assert_eq!(b_observations, 1);
}

#[tokio::test]
async fn two_stage_composition_provider_error_aborts_without_submit() {
    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    let mut value = serde_json::to_value(&config).unwrap();
    value["semantic_protocol"] = json!("m1-two-stage-v2");
    value["provider_templates"] =
        serde_json::to_value(semantic_live_provider_templates("answer-model-v1", 30)).unwrap();
    for comparison in value["run_config"]["comparisons"].as_array_mut().unwrap() {
        comparison["prompt_id"] = json!("semantic-cards-tracked-adr-m6-v12");
        comparison["schema_id"] = json!("semantic-cards-m6-json-v8");
    }
    config = serde_json::from_value(value).unwrap();
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: Some(2),
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();
    assert_eq!(result.status, LiveRunStatus::Failed);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert!(semantic.submit_count.load(Ordering::SeqCst) >= 2);
    assert!(
        !temp.path().join("artifacts/cards.jsonl").exists()
            || std::fs::read_to_string(temp.path().join("artifacts/cards.jsonl"))
                .unwrap()
                .trim()
                .is_empty()
    );
}

#[test]
fn two_stage_protocol_preflight_rejects_missing_or_unknown_mode_and_budget() {
    let temp = tempfile::tempdir().unwrap();
    let (config, _) = config(temp.path());
    let mut value = serde_json::to_value(&config).unwrap();
    value["semantic_protocol"] = json!("m1-two-stage-v2");
    value["provider_templates"] =
        serde_json::to_value(semantic_live_provider_templates("answer-model-v1", 30)).unwrap();
    for comparison in value["run_config"]["comparisons"].as_array_mut().unwrap() {
        comparison["prompt_id"] = json!("semantic-cards-tracked-adr-m6-v12");
        comparison["schema_id"] = json!("semantic-cards-m6-json-v8");
    }
    let mut good: LiveEvaluationConfig = serde_json::from_value(value).unwrap();
    assert!(validate_live_evaluation_config(&good).is_ok());
    good.provider_templates
        .retain(|template| template.stage != "composition");
    assert!(validate_live_evaluation_config(&good).is_err());
    let mut unknown = good.clone();
    unknown.semantic_protocol = "m1-unknown".into();
    assert!(validate_live_evaluation_config(&unknown).is_err());
    let mut low = serde_json::to_value(&config).unwrap();
    low["semantic_protocol"] = json!("m1-two-stage-v2");
    low["provider_templates"] =
        serde_json::to_value(semantic_live_provider_templates("answer-model-v1", 30)).unwrap();
    for comparison in low["run_config"]["comparisons"].as_array_mut().unwrap() {
        comparison["prompt_id"] = json!("semantic-cards-tracked-adr-m6-v12");
        comparison["schema_id"] = json!("semantic-cards-m6-json-v8");
    }
    low["run_config"]["comparisons"][0]["budget"]["external_request_budget"] = json!(0);
    let low: LiveEvaluationConfig = serde_json::from_value(low).unwrap();
    assert!(validate_live_evaluation_config(&low).is_err());
}

struct FakeProvider {
    calls: AtomicU32,
    fail_at: Option<u32>,
}

struct AuthFailureProvider {
    calls: AtomicU32,
}

struct GlobalErrorProvider {
    calls: AtomicU32,
    fail_at: u32,
    code: &'static str,
}

#[async_trait]
impl ProviderAppBoundary for GlobalErrorProvider {
    async fn generate(
        &self,
        _request: mcp_vault_eval::LiveProviderRequest,
    ) -> Result<LiveProviderOutput, LiveProviderError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.fail_at {
            return Err(LiveProviderError {
                code: self.code.into(),
                ..Default::default()
            });
        }
        Ok(LiveProviderOutput {
            output: json!({"claims":[{"statement":"retain rollback","evidence_indices":[1]}]}),
            usage: Some(json!({"input_tokens":10,"output_tokens":5})),
            cost_minor: Some(1),
        })
    }
}

#[async_trait]
impl ProviderAppBoundary for AuthFailureProvider {
    async fn generate(
        &self,
        _request: mcp_vault_eval::LiveProviderRequest,
    ) -> Result<LiveProviderOutput, LiveProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(LiveProviderError {
            code: "provider_auth_failed".into(),
            ..Default::default()
        })
    }
}

struct OneSchemaFailureProvider {
    calls: AtomicU32,
}

#[async_trait]
impl ProviderAppBoundary for OneSchemaFailureProvider {
    async fn generate(
        &self,
        request: mcp_vault_eval::LiveProviderRequest,
    ) -> Result<LiveProviderOutput, LiveProviderError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == 1 {
            return Err(LiveProviderError {
                code: "provider_schema_invalid".into(),
                schema_issue: Some("type_mismatch".into()),
                schema_path: Some("$.claims[0].statement".into()),
                ..Default::default()
            });
        }
        Ok(LiveProviderOutput {
            output: if request.stage == "observation" {
                json!({"claims":[{"statement":"retain rollback","evidence_indices":[1]}]})
            } else {
                json!({"answer":"supported"})
            },
            usage: Some(json!({"input_tokens":10,"output_tokens":5})),
            cost_minor: Some(1),
        })
    }
}

struct SchemaInvalidProvider {
    calls: AtomicU32,
    parse_error: bool,
}

#[async_trait]
impl ProviderAppBoundary for SchemaInvalidProvider {
    async fn generate(
        &self,
        _request: mcp_vault_eval::LiveProviderRequest,
    ) -> Result<LiveProviderOutput, LiveProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.parse_error {
            return Err(LiveProviderError {
                code: "provider_structured_json_invalid".into(),
                schema_issue: None,
                schema_path: None,
                structured_json_diagnostic: Some(mcp_vault_providers::StructuredJsonDiagnostic {
                    parser_category: mcp_vault_providers::StructuredJsonParserCategory::Eof,
                    issue: mcp_vault_providers::StructuredJsonParseIssue::UnexpectedEof,
                    line: 3,
                    column: 11,
                    content_bytes: 17,
                    parsed_bytes: 17,
                    fence_detected: false,
                    finish_reason: Some(mcp_vault_providers::StructuredJsonFinishReason::Stop),
                }),
                protocol_issue: None,
            });
        }
        Err(LiveProviderError {
            code: "provider_schema_invalid".into(),
            schema_issue: Some("enum_mismatch".into()),
            schema_path: Some("$.observations[0].body_block_indices[0]".into()),
            structured_json_diagnostic: Some(mcp_vault_providers::StructuredJsonDiagnostic {
                parser_category: mcp_vault_providers::StructuredJsonParserCategory::Syntax,
                issue: mcp_vault_providers::StructuredJsonParseIssue::GenericSyntax,
                line: 2,
                column: 4,
                content_bytes: 12,
                parsed_bytes: 12,
                fence_detected: true,
                finish_reason: Some(mcp_vault_providers::StructuredJsonFinishReason::Stop),
            }),
            protocol_issue: None,
        })
    }
}
#[async_trait]
impl ProviderAppBoundary for FakeProvider {
    async fn generate(
        &self,
        request: mcp_vault_eval::LiveProviderRequest,
    ) -> Result<LiveProviderOutput, LiveProviderError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_at == Some(call) {
            return Err(LiveProviderError {
                code: "local_fake_timeout".into(),
                ..Default::default()
            });
        }
        Ok(LiveProviderOutput {
            output: if request.stage == "observation" {
                json!({"statement":"retain rollback","content":"must not be persisted","apiKey":"x","source_text":"private","note_body":"private","request_headers":{"Authorization":"private"},"access_token":"private","refresh-token":"private"})
            } else {
                json!({"answer":"supported"})
            },
            usage: Some(
                json!({"input_tokens":10,"output_tokens":5,"authorization":"never persist"}),
            ),
            cost_minor: Some(1),
        })
    }
}

struct DelayedProvider(FakeProvider);

#[async_trait]
impl ProviderAppBoundary for DelayedProvider {
    async fn generate(
        &self,
        request: LiveProviderRequest,
    ) -> Result<LiveProviderOutput, LiveProviderError> {
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        self.0.generate(request).await
    }
}

struct LocalFailureProvider;

#[async_trait]
impl ProviderAppBoundary for LocalFailureProvider {
    async fn generate(
        &self,
        _request: LiveProviderRequest,
    ) -> Result<LiveProviderOutput, LiveProviderError> {
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        Err(LiveProviderError {
            code: "provider_endpoint_denied".into(),
            ..Default::default()
        })
    }

    fn transport_attempt_count(&self) -> Option<u32> {
        Some(0)
    }
}

#[tokio::test]
async fn generation_failure_before_http_keeps_latency_without_transport_usage() {
    for a80 in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let (mut cfg, content) = config(temp.path());
        if a80 {
            cfg.semantic_protocol = "m1-a80-v1".into();
            cfg.manifest.sources[0].logical_block_count = 2;
            cfg.provider_templates = semantic_a80_provider_templates("answer-model-v1", 30);
            for comparison in &mut cfg.run_config.comparisons {
                comparison.prompt_id = "semantic-cards-tracked-adr-m6-v14".into();
                comparison.schema_id = "semantic-cards-m6-json-v10".into();
            }
        }
        let semantic = FakeSemantic {
            prepare_count: AtomicUsize::new(0),
            submit_count: AtomicUsize::new(0),
        };
        let result = run_live_evaluation(
            &cfg,
            &Verifier { content },
            &LocalFailureProvider,
            &semantic,
        )
        .await
        .unwrap();
        assert_eq!(result.status, LiveRunStatus::Failed);
        assert_eq!(result.provider_requests, u32::from(a80));
        let usage: Value = serde_json::from_slice(
            &std::fs::read(temp.path().join("artifacts/usage.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(usage["latency"]["status"], "known");
        assert_eq!(usage["latency"]["measured"]["generation_calls"], 1);
        assert_eq!(usage["latency"]["measured"]["transport_attempts"], 0);
        assert_eq!(
            usage["latency"]["measured"]["accounted_requests"],
            u32::from(a80)
        );
        assert!(usage["latency"]["measured"]["total_ms"].as_u64().unwrap() >= 3);
        assert!(
            usage["by_arm"]
                .as_object()
                .unwrap()
                .values()
                .all(|stats| stats["request_count"] == 0)
        );
    }
}

struct InspectingProvider {
    requests: Mutex<Vec<LiveProviderRequest>>,
}

struct ConfigDriftingProvider {
    calls: AtomicU32,
    checks: AtomicUsize,
}

#[async_trait]
impl ProviderAppBoundary for ConfigDriftingProvider {
    async fn generate(
        &self,
        _request: mcp_vault_eval::LiveProviderRequest,
    ) -> Result<LiveProviderOutput, LiveProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(LiveProviderOutput {
            output: json!({"answer":"supported"}),
            usage: Some(json!({"input_tokens":11,"output_tokens":7})),
            cost_minor: Some(3),
        })
    }

    async fn verify_frozen_configuration(
        &self,
        _expected: &ProviderRuntimeSnapshot,
    ) -> Result<(), LiveProviderError> {
        if self.checks.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(())
        } else {
            Err(LiveProviderError {
                code: "provider_runtime_configuration_drift".into(),
                ..Default::default()
            })
        }
    }
}

#[async_trait]
impl ProviderAppBoundary for InspectingProvider {
    async fn generate(
        &self,
        request: LiveProviderRequest,
    ) -> Result<LiveProviderOutput, LiveProviderError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(LiveProviderOutput {
            output: if request.stage == "observation" {
                json!({"evidence_namespace":request.input["evidence_namespace"],"observations":[{"statement":"retain rollback","body_block_indices":[1],"scope":"project","kind":"decision","assertion_status":"source_asserted","admission_reason":"local fixture","value_for_future_work":"retain"}]})
            } else {
                json!({"answer":"supported"})
            },
            usage: None,
            cost_minor: None,
        })
    }
}

#[tokio::test]
async fn live_runner_persists_complete_redacted_artifacts_and_separate_usage() {
    let temp = tempfile::tempdir().unwrap();
    let (config, content) = config(temp.path());
    let provider = DelayedProvider(FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    });
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();
    assert_eq!(result.status, LiveRunStatus::Completed);
    assert_eq!(
        provider.0.calls.load(Ordering::SeqCst),
        result.provider_requests
    );
    let usage: Value =
        serde_json::from_slice(&std::fs::read(temp.path().join("artifacts/usage.json")).unwrap())
            .unwrap();
    assert_eq!(usage["latency"]["status"], "known");
    assert_eq!(
        usage["latency"]["measured"]["generation_calls"],
        result.provider_requests
    );
    assert!(usage["latency"]["measured"]["total_ms"].as_u64().unwrap() >= 3);
    assert!(usage["latency"]["measured"]["max_ms"].as_u64().unwrap() >= 3);
    assert_eq!(semantic.submit_count.load(Ordering::SeqCst), 2);
    for name in [
        "manifest.json",
        "run-config.json",
        "observations.jsonl",
        "relations.jsonl",
        "cards.jsonl",
        "packs.jsonl",
        "answers.jsonl",
        "review.jsonl",
        "usage.json",
        "report.md",
        "checkpoint.json",
    ] {
        assert!(
            temp.path().join("artifacts").join(name).is_file(),
            "missing {name}"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let artifact_root = temp.path().join("artifacts");
        assert_eq!(
            std::fs::metadata(&artifact_root)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for entry in std::fs::read_dir(&artifact_root).unwrap() {
            let entry = entry.unwrap();
            assert_eq!(
                entry.metadata().unwrap().permissions().mode() & 0o777,
                0o600,
                "sensitive artifact {} must be mode 0600",
                entry.file_name().to_string_lossy()
            );
        }
    }
    let packs = std::fs::read_to_string(temp.path().join("artifacts/packs.jsonl")).unwrap();
    assert!(packs.contains("source_references"));
    assert!(packs.contains("pack_hash"));
    assert!(packs.contains("rollback"));
    assert!(!packs.contains("note_body"));
    let cards = std::fs::read_to_string(temp.path().join("artifacts/cards.jsonl")).unwrap();
    assert!(cards.contains("retain rollback"));
    assert!(cards.contains("canonical_markdown_hash"));
    assert!(cards.contains("safe_projection_hash"));
    assert!(
        std::fs::read_to_string(temp.path().join("artifacts/answers.jsonl"))
            .unwrap()
            .contains("pack_hash")
    );
    assert!(temp.path().join("artifacts/live-config.json").is_file());
    let live_config: Value = serde_json::from_slice(
        &std::fs::read(temp.path().join("artifacts/live-config.json")).unwrap(),
    )
    .unwrap();
    for arm in ["B", "C"] {
        assert_eq!(
            live_config["observation_allowlists"][arm]["source_ids"][0],
            "S01"
        );
        assert!(
            !live_config["observation_allowlists"][arm]["source_ids_hash"]
                .as_str()
                .unwrap()
                .is_empty()
        );
        let expected_arm = if arm == "B" {
            &config.semantic_arm_roots.b
        } else {
            &config.semantic_arm_roots.c
        };
        assert_eq!(
            live_config["semantic_arm_roots"][arm]["vault_id"],
            expected_arm.vault_id
        );
        assert_eq!(
            live_config["semantic_arm_roots"][arm]["source_root"],
            expected_arm.source_root
        );
        assert!(
            !live_config["semantic_arm_roots"][arm]["source_allowlist_hash"]
                .as_str()
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(
        live_config["provider_runtime_fingerprint"],
        "frozen-runtime-fingerprint"
    );
    assert!(live_config["provider_runtime_snapshot"].is_null());
    assert!(live_config["provider_safe_settings"].is_object());
    assert_eq!(
        live_config["provider_templates"][0]["prompt_id"],
        "prompt-frozen-v1"
    );
    assert_eq!(
        live_config["provider_templates"][0]["schema_id"],
        "schema-frozen-v1"
    );
    assert_eq!(
        live_config["provider_templates"][0]["index_profile_id"],
        "index-frozen-v1"
    );
    assert_eq!(
        live_config["provider_templates"][0]["template_fingerprint"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert!(live_config["provider_templates"][0]["system"].is_null());
    assert!(live_config["provider_templates"][0]["schema"].is_null());
    let live_config_text =
        std::fs::read_to_string(temp.path().join("artifacts/live-config.json")).unwrap();
    assert!(!live_config_text.contains("frozen system"));
    assert!(!live_config_text.contains("semantic-eval"));
    assert!(!live_config_text.contains("provider.invalid"));
    let review = std::fs::read_to_string(temp.path().join("artifacts/review.jsonl")).unwrap();
    assert!(review.contains("ordinary_lexical_coverage"));
    assert!(review.contains("current_indexed_source_count"));
    let observations =
        std::fs::read_to_string(temp.path().join("artifacts/observations.jsonl")).unwrap();
    assert!(observations.contains("retain rollback"));
    assert!(!observations.contains("must not be persisted"));
    assert!(!observations.contains("authorization"));
    assert!(!observations.contains("apiKey"));
    assert!(!observations.contains("source_text"));
    assert!(!observations.contains("note_body"));
    assert!(!observations.contains("access_token"));
    let usage: Value =
        serde_json::from_slice(&std::fs::read(temp.path().join("artifacts/usage.json")).unwrap())
            .unwrap();
    assert_eq!(usage["by_arm"]["B"]["request_count"], 2);
    assert_eq!(usage["by_arm"]["C"]["request_count"], 3);
    let report = std::fs::read_to_string(temp.path().join("artifacts/report.md")).unwrap();
    assert!(report.contains("not_evaluated"));
}

#[tokio::test]
async fn successful_provider_usage_is_persisted_before_postcall_config_drift_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let (config, content) = config(temp.path());
    let provider = ConfigDriftingProvider {
        calls: AtomicU32::new(0),
        checks: AtomicUsize::new(0),
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();
    assert_eq!(result.status, LiveRunStatus::Failed);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        result.first_error_code.as_deref(),
        Some("provider_runtime_configuration_drift")
    );
    let usage: Value =
        serde_json::from_slice(&std::fs::read(temp.path().join("artifacts/usage.json")).unwrap())
            .unwrap();
    assert_eq!(usage["request_count"], 1);
    assert_eq!(usage["by_arm"]["B"]["input_tokens"], 11);
    assert_eq!(usage["by_arm"]["B"]["output_tokens"], 7);
    assert_eq!(usage["by_arm"]["B"]["cost_minor"], 3);
    let observations =
        std::fs::read_to_string(temp.path().join("artifacts/observations.jsonl")).unwrap();
    assert!(observations.contains("input_tokens"));
}

#[tokio::test]
async fn live_transport_request_budget_caps_retry_attempt_reservations() {
    let budget = mcp_vault_eval::LiveTransportRequestBudget::new(2);
    mcp_vault_providers::RequestBudget::reserve(&budget, 100)
        .await
        .unwrap();
    mcp_vault_providers::RequestBudget::reserve(&budget, 100)
        .await
        .unwrap();
    assert!(matches!(
        mcp_vault_providers::RequestBudget::reserve(&budget, 100).await,
        Err(mcp_vault_providers::ProviderError::RequestBudgetExhausted)
    ));
    assert_eq!(budget.used(), 2);
}

#[tokio::test]
async fn live_transport_budget_stops_transient_http_retries_at_the_attempt_limit() {
    use axum::{Router, extract::State, http::StatusCode, routing::post};
    use std::{sync::Arc, sync::atomic::AtomicU32};
    use tokio::net::TcpListener;

    async fn transient(State(attempts): State<Arc<AtomicU32>>) -> StatusCode {
        attempts.fetch_add(1, Ordering::SeqCst);
        StatusCode::SERVICE_UNAVAILABLE
    }

    let attempts = Arc::new(AtomicU32::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server_attempts = attempts.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/retry", post(transient))
                .with_state(server_attempts),
        )
        .await
        .unwrap();
    });
    let request_budget = mcp_vault_eval::LiveTransportRequestBudget::new(2);
    let transport =
        mcp_vault_providers::ProviderTransport::new(mcp_vault_providers::ProviderSettings {
            max_retries: 5,
            ..Default::default()
        })
        .unwrap()
        .with_budget(std::sync::Arc::new(request_budget.clone()));
    let endpoint = url::Url::parse(&format!("http://{address}/retry")).unwrap();
    let error = transport
        .request_json(
            reqwest::Method::POST,
            &endpoint,
            mcp_vault_providers::ProviderMode::Enabled,
            &json!({"local_fixture":true}),
            mcp_vault_providers::RequestOptions::new(mcp_vault_providers::AuthStyle::None, None),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        mcp_vault_providers::ProviderError::RequestBudgetExhausted
    ));
    assert_eq!(request_budget.used(), 2);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn relation_and_answer_receive_the_same_nonempty_typed_safe_pack() {
    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    let task_query = config.manifest.tasks[0].query.clone();
    config.manifest.tasks[0].must_preserve = vec!["gold-only-preserve-marker".into()];
    config.manifest.tasks[0].must_not_infer = vec!["gold-only-forbidden-marker".into()];
    config.manifest.tasks[0].expected_usable_information = vec!["gold-only-usable-marker".into()];
    config.manifest.tasks[0].expected_status = "gold-only-status-marker".into();
    let provider = InspectingProvider {
        requests: Mutex::new(Vec::new()),
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();
    assert_eq!(result.status, LiveRunStatus::Completed);
    let requests = provider.requests.lock().unwrap();
    let relation = requests
        .iter()
        .find(|request| request.stage == "relation")
        .unwrap();
    let c_answer = requests
        .iter()
        .find(|request| request.stage == "answer" && request.arm == ComparisonArm::C)
        .unwrap();
    let answer_requests = requests
        .iter()
        .filter(|request| request.stage == "answer")
        .collect::<Vec<_>>();
    assert_eq!(answer_requests.len(), 3, "A, B, and C each answer once");
    assert_eq!(
        answer_requests
            .iter()
            .map(|request| request.arm.clone())
            .collect::<Vec<_>>(),
        vec![ComparisonArm::A, ComparisonArm::B, ComparisonArm::C]
    );
    for answer in answer_requests {
        assert_eq!(answer.input["query"], task_query);
        let serialized_input = answer.input.to_string();
        for marker in [
            "gold-only-preserve-marker",
            "gold-only-forbidden-marker",
            "gold-only-usable-marker",
            "gold-only-status-marker",
            "must_preserve",
            "must_not_infer",
            "expected_usable_information",
            "expected_status",
            "expected_source_relations",
            "severity",
        ] {
            assert!(
                !serialized_input.contains(marker),
                "gold field/value {marker} must remain local"
            );
        }
    }
    for arm in [ComparisonArm::B, ComparisonArm::C] {
        let answer = requests
            .iter()
            .find(|request| request.stage == "answer" && request.arm == arm)
            .unwrap();
        assert_eq!(answer.input["pack"], c_answer.input["pack"]);
        assert_eq!(answer.input["pack_hash"], c_answer.input["pack_hash"]);
        assert!(answer.input["pack"].get("query").is_none());
    }
    let safe_pack = &relation.input["pack"];
    assert_eq!(safe_pack["current_context"].as_array().unwrap().len(), 1);
    assert_eq!(
        safe_pack["current_context"][0]["core_assertions"][0],
        "rollback"
    );
    assert_eq!(
        safe_pack["current_context"][0]["conditions"][0],
        "rollback approved"
    );
    assert_eq!(
        safe_pack["current_context"][0]["evidence_refs"][0],
        "evidence-1"
    );
    assert_eq!(safe_pack["related_sources"].as_array().unwrap().len(), 0);
    assert!(!safe_pack.to_string().contains("note_body"));
    assert_eq!(relation.input["pack_hash"], c_answer.input["pack_hash"]);
    assert_eq!(relation.input["pack"], c_answer.input["pack"]);
    let persisted_packs =
        std::fs::read_to_string(temp.path().join("artifacts/packs.jsonl")).unwrap();
    assert_eq!(
        persisted_packs.lines().count(),
        2,
        "B and C each build exactly one pack"
    );
}

#[tokio::test]
async fn arm_copy_drift_after_pack_stops_relation_and_checkpoints_failure() {
    let temp = tempfile::tempdir().unwrap();
    let (config, content) = config(temp.path());
    let provider = InspectingProvider {
        requests: Mutex::new(Vec::new()),
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    semantic.drift_after_pack();
    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();
    assert_eq!(result.status, LiveRunStatus::Failed);
    assert_eq!(result.first_error_code.as_deref(), Some("source_mismatch"));
    assert!(
        !provider
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.stage == "relation")
    );
    let checkpoint: Value = serde_json::from_slice(
        &std::fs::read(temp.path().join("artifacts/checkpoint.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        checkpoint["first_error"]["stage"],
        "arm_source_fence_after_pack"
    );
}

#[tokio::test]
async fn ordinary_lexical_zero_candidates_continue_to_answer_and_record_quality_event() {
    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    config.manifest.tasks[0].query = "force-empty-index-results".into();
    config.manifest.tasks[0].expected_no_answer = true;
    config.manifest.tasks[0].expected_status = "no_answer".into();
    let provider = InspectingProvider {
        requests: Mutex::new(Vec::new()),
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();
    assert_ne!(result.status, LiveRunStatus::Failed);
    assert!(result.first_error_code.is_none());
    let requests = provider.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .any(|request| request.stage == "answer" && request.arm == ComparisonArm::A)
    );
    let answer_requests = requests
        .iter()
        .filter(|request| request.stage == "answer")
        .collect::<Vec<_>>();
    assert_eq!(answer_requests.len(), 3);
    assert!(answer_requests.iter().all(|request| {
        request.input["query"] == "force-empty-index-results"
            && !request.input.to_string().contains("no_answer")
    }));
    let review = std::fs::read_to_string(temp.path().join("artifacts/review.jsonl")).unwrap();
    assert!(review.contains("ordinary_lexical_coverage"));
    assert!(review.contains("\"eligible_count\":0"));
}

#[tokio::test]
async fn live_runner_fails_closed_and_checkpoints_when_source_drifts_after_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let (config, content) = config(temp.path());
    let verifier = DriftingVerifier {
        inner: Verifier { content },
        drift_after: 4,
        calls: AtomicUsize::new(0),
    };
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let result = run_live_evaluation(&config, &verifier, &provider, &semantic)
        .await
        .unwrap();
    assert_eq!(result.status, LiveRunStatus::Failed);
    assert_eq!(result.first_error_code.as_deref(), Some("source_mismatch"));
    let checkpoint: Value = serde_json::from_slice(
        &std::fs::read(temp.path().join("artifacts/checkpoint.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(checkpoint["status"], "failed");
    assert_eq!(checkpoint["first_error"]["code"], "source_mismatch");
}

#[tokio::test]
async fn live_runner_stops_at_first_provider_error_and_keeps_checkpoint() {
    let temp = tempfile::tempdir().unwrap();
    let (config, content) = config(temp.path());
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: Some(2),
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();
    assert_eq!(result.status, LiveRunStatus::Failed);
    assert_eq!(
        result.first_error_code.as_deref(),
        Some("local_fake_timeout")
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    let checkpoint: Value = serde_json::from_slice(
        &std::fs::read(temp.path().join("artifacts/checkpoint.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(checkpoint["first_error"]["code"], "local_fake_timeout");
    assert!(
        std::fs::read_to_string(temp.path().join("artifacts/answers.jsonl"))
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn live_runner_persists_safe_schema_diagnostic_with_attempt_context() {
    let temp = tempfile::tempdir().unwrap();
    let (config, content) = config(temp.path());
    let provider = SchemaInvalidProvider {
        calls: AtomicU32::new(0),
        parse_error: false,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();
    assert_eq!(result.status, LiveRunStatus::Failed);
    assert_eq!(
        result.first_error_code.as_deref(),
        Some("provider_schema_invalid")
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(semantic.submit_count.load(Ordering::SeqCst), 1);
    let attempts = std::fs::read_to_string(temp.path().join("artifacts/attempts.jsonl")).unwrap();
    assert!(attempts.contains("\"status\":\"started\""));
    assert!(attempts.contains("\"stage\":\"observation\""));
    let diagnostics =
        std::fs::read_to_string(temp.path().join("artifacts/schema-diagnostics.jsonl")).unwrap();
    assert!(diagnostics.contains("\"issue\":\"enum_mismatch\""));
    assert!(diagnostics.contains("$.observations[0].body_block_indices[0]"));
    assert!(diagnostics.contains("\"structured_json_diagnostic\""));
    assert!(diagnostics.contains("\"fence_detected\":true"));
    assert!(diagnostics.contains("\"arm\":\"b\""));
    assert!(diagnostics.contains("\"source_id\":\"S01\""));
    assert!(diagnostics.contains("\"task_id\":null"));
    assert!(!diagnostics.contains("private") && !diagnostics.contains("Authorization"));
    let report = std::fs::read_to_string(temp.path().join("artifacts/report.md")).unwrap();
    assert!(report.contains("enum_mismatch"));
    assert!(report.contains("$.observations[0].body_block_indices[0]"));
    assert!(report.contains("structured_json_diagnostic"));
}

#[tokio::test]
async fn live_runner_routes_pure_structured_json_diagnostic_without_schema_fields() {
    let temp = tempfile::tempdir().unwrap();
    let (config, content) = config(temp.path());
    let provider = SchemaInvalidProvider {
        calls: AtomicU32::new(0),
        parse_error: true,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let result = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap();
    assert_eq!(
        result.first_error_code.as_deref(),
        Some("provider_structured_json_invalid")
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let diagnostics =
        std::fs::read_to_string(temp.path().join("artifacts/schema-diagnostics.jsonl")).unwrap();
    assert!(diagnostics.contains("\"parser_category\":\"eof\""));
    assert!(diagnostics.contains("\"schema_issue\":null") || !diagnostics.contains("schema_issue"));
    assert!(diagnostics.contains("\"arm\":\"b\"") && diagnostics.contains("\"source_id\":\"S01\""));
    let report = std::fs::read_to_string(temp.path().join("artifacts/report.md")).unwrap();
    assert!(report.contains("provider_structured_json_invalid"));
    assert!(report.contains("structured_json_diagnostic"));
}

#[tokio::test]
async fn live_runner_rejects_missing_authorization_before_provider_call() {
    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    config.run_config.explicit_live_authorization = false;
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let error = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap_err();
    assert!(matches!(error, mcp_vault_eval::EvalError::LiveBlocked(_)));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn live_runner_rejects_missing_stage_and_records_failed_preflight() {
    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    config
        .provider_templates
        .retain(|template| template.stage != "relation");
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let error = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap_err();
    assert!(matches!(error, mcp_vault_eval::EvalError::LiveBlocked(_)));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    let checkpoint: Value = serde_json::from_slice(
        &std::fs::read(temp.path().join("artifacts/checkpoint.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(checkpoint["status"], "failed");
}

#[tokio::test]
async fn public_runner_does_not_checkpoint_a_preflight_error_into_overlapping_roots() {
    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    config
        .provider_templates
        .retain(|template| template.stage != "relation");
    config.artifact_root = config.state_root.clone();
    config.run_config.artifact_root = Some(config.state_root.clone());
    let state_root = Path::new(&config.state_root);
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };

    let error = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        mcp_vault_eval::EvalError::LiveConfig(_) | mcp_vault_eval::EvalError::ShadowConfig(_)
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert!(
        !state_root.exists(),
        "preflight error must not create a checkpoint under state_root"
    );
}

#[tokio::test]
async fn live_runner_requires_one_exact_task_set_for_all_arms() {
    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    config.run_config.comparisons[1].task_ids.clear();
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let error = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap_err();
    assert!(matches!(error, mcp_vault_eval::EvalError::LiveBlocked(_)));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn live_runner_requires_holdout_and_disjoint_arm_roots_before_provider_call() {
    let temp = tempfile::tempdir().unwrap();
    let (mut non_holdout_config, content) = config(temp.path());
    non_holdout_config.manifest.tasks[0].split = TaskSplit::Development;
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    assert!(
        run_live_evaluation(
            &non_holdout_config,
            &Verifier { content },
            &provider,
            &semantic,
        )
        .await
        .is_err()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);

    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    config.semantic_arm_roots.c.state_root = config.semantic_arm_roots.b.state_root.clone();
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    };
    let error = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap_err();
    assert!(matches!(error, mcp_vault_eval::EvalError::LiveConfig(_)));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn live_manifest_rejects_split_source_overlap_and_partial_holdout_task_sets() {
    let temp = tempfile::tempdir().unwrap();
    let (mut split_overlap, _) = config(temp.path());
    let mut development = split_overlap.manifest.tasks[0].clone();
    development.id = "Q02-development".into();
    development.query_refs = vec!["Q02-ref".into()];
    development.split = TaskSplit::Development;
    split_overlap.manifest.tasks.push(development);
    assert!(validate_live_evaluation_config(&split_overlap).is_err());

    let temp = tempfile::tempdir().unwrap();
    let (mut partial_holdout, _) = config(temp.path());
    let mut second_holdout = partial_holdout.manifest.tasks[0].clone();
    second_holdout.id = "Q02-holdout".into();
    second_holdout.query_refs = vec!["Q02-ref".into()];
    partial_holdout.manifest.tasks.push(second_holdout);
    assert!(validate_live_evaluation_config(&partial_holdout).is_err());
}

#[tokio::test]
async fn live_runner_rejects_development_and_unselected_task_sources_before_provider_call() {
    let temp = tempfile::tempdir().unwrap();
    let (mut dev_config, content) = config(temp.path());
    let dev_body = b"development-only source";
    let dev = extra_source("S-DEV", "file-dev", "notes/dev.md", dev_body);
    dev_config.manifest.sources.push(dev);
    dev_config
        .run_config
        .source_allowlist
        .extend(["S-DEV".into(), "file-dev".into()]);
    for arm in &mut dev_config.run_config.comparisons[1..] {
        arm.source_ids.push("S-DEV".into());
    }
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    assert!(
        run_live_evaluation(&dev_config, &Verifier { content }, &provider, &semantic)
            .await
            .is_err()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);

    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    let outside_body = b"source from an unselected holdout task";
    let outside = extra_source("S02", "file-real-2", "notes/other-holdout.md", outside_body);
    config.manifest.sources.push(outside.clone());
    config.manifest.tasks.push(EvalTask {
        id: "Q02".into(),
        split: TaskSplit::Holdout,
        query_refs: vec!["Q02-support".into()],
        query: "A separate holdout task not selected for this run".into(),
        source_ids: vec!["S02".into()],
        source_fence: vec![mcp_vault_eval::TaskSourceFence {
            source_id: "S02".into(),
            file_revision: outside.file_revision,
            source_revision_id: outside.source_revision_id.clone(),
            content_hash: outside.content_hash.clone(),
        }],
        must_preserve: Vec::new(),
        must_not_infer: Vec::new(),
        expected_usable_information: Vec::new(),
        expected_no_answer: true,
        expected_status: "no_answer".into(),
        expected_source_relations: Vec::new(),
        severity: "normal".into(),
    });
    config
        .run_config
        .source_allowlist
        .extend(["S02".into(), "file-real-2".into()]);
    for arm in &mut config.run_config.comparisons[1..] {
        arm.source_ids.push("S02".into());
    }
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    assert!(
        run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
            .await
            .is_err()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn isolated_master_key_must_remain_inside_non_symlink_run_root() {
    let temp = tempfile::tempdir().unwrap();
    let run_root = temp.path().join("run");
    std::fs::create_dir_all(&run_root).unwrap();
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    assert!(
        validate_isolated_master_key_path(run_root.to_str().unwrap(), &outside.join("master.key"))
            .is_err()
    );
    #[cfg(unix)]
    {
        let alias = run_root.join("alias");
        std::os::unix::fs::symlink(&outside, &alias).unwrap();
        assert!(
            validate_isolated_master_key_path(
                run_root.to_str().unwrap(),
                &alias.join("master.key")
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn live_runner_rejects_source_root_outside_isolated_run_root() {
    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    config.source_root = temp
        .path()
        .parent()
        .unwrap()
        .join("external-source-root")
        .display()
        .to_string();
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let error = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap_err();
    assert!(matches!(error, mcp_vault_eval::EvalError::LiveConfig(_)));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn external_model_identity_mismatch_is_rejected_before_adapter_use() {
    let temp = tempfile::tempdir().unwrap();
    let (config, _) = config(temp.path());
    let template = ProviderStageTemplate {
        stage: "answer".into(),
        model_id: "answer-model-v1".into(),
        prompt_id: "prompt-frozen-v1".into(),
        schema_id: "schema-frozen-v1".into(),
        index_profile_id: "index-frozen-v1".into(),
        system: "frozen".into(),
        schema_name: "answer".into(),
        schema: json!({"type":"object"}),
        max_output_tokens: 64,
        temperature: Some(0.0),
        timeout_seconds: 30,
    };
    let error =
        validate_external_model_identity(&config.run_config, &[template], "different-model")
            .unwrap_err();
    assert!(matches!(error, mcp_vault_eval::EvalError::LiveBlocked(_)));
}

#[tokio::test]
async fn returned_cost_budget_stops_without_retrying() {
    let temp = tempfile::tempdir().unwrap();
    let (mut config, content) = config(temp.path());
    config.run_config.cost_budget_minor = 1;
    config.unbounded_cost_authorized = false;
    let provider = FakeProvider {
        calls: AtomicU32::new(0),
        fail_at: None,
    };
    let semantic = FakeSemantic {
        prepare_count: AtomicUsize::new(0),
        submit_count: AtomicUsize::new(0),
    };
    let error = run_live_evaluation(&config, &Verifier { content }, &provider, &semantic)
        .await
        .unwrap_err();
    assert!(matches!(error, mcp_vault_eval::EvalError::LiveBlocked(_)));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    let checkpoint: Value = serde_json::from_slice(
        &std::fs::read(temp.path().join("artifacts/checkpoint.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(checkpoint["status"], "failed");
}
