//! Prepare a private, isolated M6 A/B/C evaluation from tracked repository ADRs.
//!
//! This binary performs no network requests. It reads the selected Provider
//! configuration through State/Auth repositories, copies the encrypted secret
//! into the isolated baseline State using a new run key, imports the exact
//! committed Markdown through Vault Core, and writes a frozen live config.

use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_core::VaultCore;
use mcp_vault_domain::{
    Actor, Revision, SourcePlane, VaultContext, VaultId, VaultPath, VaultPathPolicy, VaultSlug,
    WritePrecondition,
};
use mcp_vault_eval::{
    Budget, ComparisonArm, ComparisonConfig, EvalMode, EvalSource, EvalTask, EvaluationManifest,
    EvaluationRunConfig, ExpectedRelation, LIVE_RUN_SCHEMA, LiveEvaluationConfig, M6_A80_PROMPT_ID,
    M6_A80_SCHEMA_ID, ProviderStageTemplate, SemanticArmRoot, SemanticArmRoots, TaskSourceFence,
    TaskSplit, prepare_live_private_directories, semantic_a80_provider_templates,
    validate_live_evaluation_config, write_live_preparation_seal,
};
use mcp_vault_indexer::IndexService;
use mcp_vault_memory::SemanticMemoryService;
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, PROVIDER_SECRET_OWNER, PROVIDER_SECRET_PURPOSE,
    ProviderInput, ProviderKind, ProviderMode, ProviderService, ProviderSettings,
};
use mcp_vault_state::{ModelRecord, ProviderRecord, StateStore, VaultStatus};
use mcp_vault_storage_fs::{DurabilityPolicy, StorageOptions};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fs::{self, DirBuilder, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    process::Command,
};
use tokio::io::AsyncWriteExt;
use url::Url;
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
const PREPARE_FLAG: &str = "--prepare-authorized-real-semantic-evaluation";
const MANIFEST_SCHEMA: &str = "semantic-memory-eval-manifest-v1";
const RUN_CONFIG_SCHEMA: &str = "semantic-memory-eval-run-config-v1";
const EXTRACTION_PROFILE: &str = "semantic-memory-m1-v1";
const PROVIDER_ROLE: &str = "memory_extraction";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftConfig {
    run_root: PathBuf,
    repository_root: PathBuf,
    source_database_path: PathBuf,
    source_master_key_path: PathBuf,
    #[serde(default)]
    source_vault_slug: Option<String>,
    sources: Vec<SourceDraft>,
    tasks: Vec<TaskDraft>,
    b_source_ids: Vec<String>,
    external_request_budget: u32,
    #[serde(default)]
    provider_timeout_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceDraft {
    logical_id: String,
    path: String,
    split: TaskSplit,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskDraft {
    id: String,
    split: TaskSplit,
    query_refs: Vec<String>,
    query: String,
    source_ids: Vec<String>,
    must_preserve: Vec<String>,
    must_not_infer: Vec<String>,
    expected_usable_information: Vec<String>,
    expected_no_answer: bool,
    expected_status: String,
    expected_source_relations: Vec<ExpectedRelation>,
    severity: String,
}

struct SourceMaterial {
    draft: SourceDraft,
    bytes: Vec<u8>,
    content_hash: String,
}

struct ArmFixture {
    state: StateStore,
    context: VaultContext,
    core: VaultCore,
    source_root: PathBuf,
    state_root: PathBuf,
    history_root: PathBuf,
}

struct FrozenProvider {
    source_vault_slug: String,
    source_provider_type: String,
    source_model_id: String,
    external_model_id: String,
    provider_kind: ProviderKind,
    base_url: Url,
    settings: ProviderSettings,
    capabilities: ModelCapabilities,
    model_settings: ModelSettings,
    secret: mcp_vault_auth::SecretString,
}

fn usage() -> &'static str {
    "usage: prepare-semantic-card-live-eval --prepare-authorized-real-semantic-evaluation DRAFTS_JSON"
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    if args.next().as_deref() != Some(PREPARE_FLAG) {
        return Err(format!("{}\nexplicit M6 preparation flag is required", usage()).into());
    }
    let Some(drafts_path) = args.next() else {
        return Err(usage().into());
    };
    if args.next().is_some() {
        return Err(usage().into());
    }
    let drafts_path = PathBuf::from(drafts_path);
    let draft: DraftConfig = serde_json::from_slice(&fs::read(&drafts_path)?)?;
    validate_draft(&draft)?;
    validate_private_run_root(&draft.run_root)?;

    // Read the frozen, committed source bytes before opening Provider state.
    let git_revision = git_output(&draft.repository_root, &["rev-parse", "HEAD"])?;
    let sources = read_committed_sources(&draft.repository_root, &draft.sources)?;
    let provider = read_current_extraction_provider(&draft).await?;

    let (baseline, b_arm, c_arm, master_key_path) = create_isolated_fixtures(&draft).await?;
    let isolated_keys = load_or_create_private_master_key(&master_key_path).await?;
    let isolated_auth = AuthService::new(baseline.state.auth(), isolated_keys);
    let provider_service = ProviderService::new(baseline.state.clone(), isolated_auth);
    provider_service
        .set_provider_mode(&baseline.context, ProviderMode::RemoteAllowed, None)
        .await?;

    let timeout_seconds = draft
        .provider_timeout_seconds
        .unwrap_or(provider.settings.timeout_ms.div_ceil(1000));
    let timeout_ms = timeout_seconds
        .checked_mul(1_000)
        .ok_or("provider timeout seconds overflow")?;
    let mut isolated_settings = provider.settings.clone();
    isolated_settings.timeout_ms = timeout_ms;
    isolated_settings.max_retries = 0;
    isolated_settings.max_concurrency = 1;
    isolated_settings.validate()?;
    let isolated_provider = provider_service
        .create_provider(ProviderInput {
            name: "M6 isolated semantic evaluation".to_owned(),
            kind: provider.provider_kind,
            base_url: provider.base_url.clone(),
            settings: isolated_settings,
            enabled: true,
            secret: Some(provider.secret),
        })
        .await?;
    let isolated_model = provider_service
        .register_model(ModelInput {
            provider_id: isolated_provider.id,
            external_model_id: provider.external_model_id.clone(),
            capabilities: provider.capabilities.clone(),
            settings: provider.model_settings.clone(),
            enabled: true,
        })
        .await?;
    let provider_snapshot = provider_service
        .runtime_snapshot(&baseline.context, isolated_model.id)
        .await?;

    let baseline_sources = import_sources(&baseline, &sources).await?;
    import_sources(&b_arm, &sources).await?;
    import_sources(&c_arm, &sources).await?;
    IndexService::new(baseline.state.clone())
        .rebuild_vault(&baseline.core, &baseline.context)
        .await?;

    let task_map = draft
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect::<BTreeMap<_, _>>();
    let manifest_sources = draft
        .sources
        .iter()
        .map(|source| {
            let source = baseline_sources
                .get(&source.logical_id)
                .cloned()
                .ok_or_else(|| std::io::Error::other("baseline source metadata is missing"))?;
            Ok::<_, Box<dyn Error + Send + Sync>>(source)
        })
        .collect::<Result<Vec<_>>>()?;
    let source_by_id = manifest_sources
        .iter()
        .map(|source| (source.logical_id.clone(), source))
        .collect::<BTreeMap<_, _>>();
    let tasks = draft
        .tasks
        .iter()
        .map(|task| {
            let source_fence = task
                .source_ids
                .iter()
                .map(|source_id| {
                    let source = source_by_id
                        .get(source_id)
                        .ok_or("task references an unknown source")?;
                    Ok(TaskSourceFence {
                        source_id: source.logical_id.clone(),
                        file_revision: source.file_revision,
                        source_revision_id: source.source_revision_id.clone(),
                        content_hash: source.content_hash.clone(),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(EvalTask {
                id: task.id.clone(),
                split: task.split.clone(),
                query_refs: task.query_refs.clone(),
                query: task.query.clone(),
                source_ids: task.source_ids.clone(),
                source_fence,
                must_preserve: task.must_preserve.clone(),
                must_not_infer: task.must_not_infer.clone(),
                expected_usable_information: task.expected_usable_information.clone(),
                expected_no_answer: task.expected_no_answer,
                expected_status: task.expected_status.clone(),
                expected_source_relations: task.expected_source_relations.clone(),
                severity: task.severity.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if task_map.len() != draft.tasks.len() {
        return Err("evaluation task IDs are duplicated".into());
    }

    let manifest = EvaluationManifest {
        schema_version: MANIFEST_SCHEMA.to_owned(),
        dataset_id: format!(
            "mcp-vault-tracked-adr-m6-{}",
            git_revision.chars().take(12).collect::<String>()
        ),
        synthetic_only: false,
        sources: manifest_sources.clone(),
        tasks,
    };
    // Streaming generations use the bounded 600s total deadline while the
    // Provider runtime keeps 120s first-event/idle defaults.
    let templates = semantic_a80_provider_templates(&provider.external_model_id, 600);
    let budget = Budget {
        max_entries: 16,
        max_bytes: 64_000,
        max_tokens: 4_096,
        external_request_budget: draft.external_request_budget,
    };
    let holdout_task_ids = manifest
        .tasks
        .iter()
        .filter(|task| task.split == TaskSplit::Holdout)
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    let source_ids = manifest
        .sources
        .iter()
        .map(|source| source.logical_id.clone())
        .collect::<Vec<_>>();
    let holdout_source_ids = draft
        .sources
        .iter()
        .filter(|source| source.split == TaskSplit::Holdout)
        .map(|source| source.logical_id.clone())
        .collect::<Vec<_>>();
    let source_allowlist = manifest
        .sources
        .iter()
        .flat_map(|source| [source.logical_id.clone(), source.file_id.clone()])
        .collect::<Vec<_>>();
    let make_comparison = |arm, arm_source_ids: Vec<String>| ComparisonConfig {
        arm,
        source_ids: arm_source_ids,
        task_ids: holdout_task_ids.clone(),
        budget: budget.clone(),
        index_profile_id: "index-frozen-v1".to_owned(),
        prompt_id: M6_A80_PROMPT_ID.to_owned(),
        schema_id: M6_A80_SCHEMA_ID.to_owned(),
        answer_model_id: provider.external_model_id.clone(),
    };
    let artifact_root = draft.run_root.join("artifacts");
    let evaluation = LiveEvaluationConfig {
        schema_version: LIVE_RUN_SCHEMA.to_owned(),
        semantic_protocol: "m1-a80-v1".to_owned(),
        manifest,
        run_config: EvaluationRunConfig {
            schema_version: RUN_CONFIG_SCHEMA.to_owned(),
            mode: EvalMode::Live,
            allow_live: true,
            explicit_live_authorization: true,
            source_allowlist,
            cost_budget_minor: 0,
            artifact_root: Some(artifact_root.display().to_string()),
            comparisons: vec![
                make_comparison(ComparisonArm::A, source_ids),
                make_comparison(ComparisonArm::B, draft.b_source_ids.clone()),
                make_comparison(ComparisonArm::C, holdout_source_ids),
            ],
        },
        run_root: draft.run_root.display().to_string(),
        source_root: baseline.source_root.display().to_string(),
        state_root: baseline.state_root.display().to_string(),
        history_root: baseline.history_root.display().to_string(),
        artifact_root: artifact_root.display().to_string(),
        isolated_vault_id: baseline.context.id().to_string(),
        semantic_arm_roots: SemanticArmRoots {
            b: arm_root(&b_arm),
            c: arm_root(&c_arm),
        },
        current_schema: LIVE_RUN_SCHEMA.to_owned(),
        task_budget: u32::try_from(holdout_task_ids.len())?,
        unbounded_cost_authorized: true,
        provider_model_id: Some(isolated_model.id.to_string()),
        provider_runtime_snapshot: Some(provider_snapshot),
        provider_templates: templates.clone(),
        isolated_master_key_path: Some(master_key_path.display().to_string()),
    };
    validate_live_evaluation_config(&evaluation)?;
    write_live_preparation_seal(&evaluation, &master_key_path)?;
    prepare_live_private_directories(&evaluation, &master_key_path)?;
    validate_live_evaluation_config(&evaluation)?;

    let cli_config = CliConfig {
        evaluation: evaluation.clone(),
        vault_slug: baseline.context.slug().as_str().to_owned(),
        master_key_path: master_key_path.display().to_string(),
        model_id: isolated_model.id.to_string(),
        templates,
    };
    write_private_json(&draft.run_root.join("live-config.json"), &cli_config)?;
    let provenance = json!({
        "schema_version": LIVE_RUN_SCHEMA,
        "dataset_id": evaluation.manifest.dataset_id,
        "source_repository_commit": git_revision,
        "source_count": evaluation.manifest.sources.len(),
        "task_count": evaluation.manifest.tasks.len(),
        "development_sources": evaluation.manifest.sources.iter().filter(|source| source_split(source.logical_id.as_str(), &draft.sources) == Some(TaskSplit::Development)).count(),
        "holdout_sources": evaluation.manifest.sources.iter().filter(|source| source_split(source.logical_id.as_str(), &draft.sources) == Some(TaskSplit::Holdout)).count(),
        "development_tasks": evaluation.manifest.tasks.iter().filter(|task| task.split == TaskSplit::Development).count(),
        "holdout_tasks": evaluation.manifest.tasks.iter().filter(|task| task.split == TaskSplit::Holdout).count(),
        "difficult_tasks": evaluation.manifest.tasks.iter().filter(|task| matches!(task.severity.as_str(), "high" | "critical")).count(),
        "provider_type": provider.source_provider_type,
        "source_model_id": provider.source_model_id,
        "external_model_id": provider.external_model_id,
        "provider_retries": 0,
        "external_request_budget": draft.external_request_budget,
        "currency_budget": "unbounded_by_user_authorization",
        "embedding_role_used": false,
        "retrieval_profile": "index-frozen-v1 lexical",
        "source_allowlist": evaluation.manifest.sources.iter().map(|source| json!({"id":source.logical_id,"path":source.path,"sha256":source.content_hash})).collect::<Vec<_>>(),
        "provider_source_vault_slug": provider.source_vault_slug,
        "expected_call_count_upper_bound": draft.external_request_budget,
        "cost_reporting": "Provider adapter returns token usage but no per-call price; currency cost remains unavailable unless the Provider response supplies it.",
    });
    write_private_json(&draft.run_root.join("provenance.json"), &provenance)?;

    println!(
        "{}",
        serde_json::to_string(&json!({
            "prepared": true,
            "run_root": draft.run_root,
            "config": draft.run_root.join("live-config.json"),
            "manifest_hash": mcp_vault_eval::validate_live_evaluation_config(&evaluation).map(|(manifest, _)| manifest).unwrap_or_default(),
            "source_count": evaluation.manifest.sources.len(),
            "task_count": evaluation.manifest.tasks.len(),
            "holdout_task_count": holdout_task_ids.len(),
            "difficult_task_count": evaluation.manifest.tasks.iter().filter(|task| matches!(task.severity.as_str(), "high" | "critical")).count(),
            "provider_type": provider.source_provider_type,
            "external_model_id": provider.external_model_id,
            "external_request_budget": draft.external_request_budget,
            "retries": 0,
            "real_provider_requests_started": 0,
        }))?
    );
    Ok(())
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CliConfig {
    evaluation: LiveEvaluationConfig,
    vault_slug: String,
    master_key_path: String,
    model_id: String,
    templates: Vec<ProviderStageTemplate>,
}

fn validate_draft(config: &DraftConfig) -> Result<()> {
    if !config.run_root.is_absolute()
        || !config.repository_root.is_absolute()
        || !config.source_database_path.is_absolute()
        || !config.source_master_key_path.is_absolute()
        || config.external_request_budget == 0
        || config.sources.len() != 30
        || config.tasks.len() != 60
        || config
            .tasks
            .iter()
            .filter(|task| task.split == TaskSplit::Holdout)
            .count()
            != 30
        || config
            .sources
            .iter()
            .filter(|source| source.split == TaskSplit::Holdout)
            .count()
            != 15
        || config
            .sources
            .iter()
            .filter(|source| source.split == TaskSplit::Development)
            .count()
            != 15
        || config
            .tasks
            .iter()
            .filter(|task| matches!(task.severity.as_str(), "high" | "critical"))
            .count()
            < 20
    {
        return Err("M6 draft must have 30 tracked sources, 60 tasks, 30 holdout tasks, and 20 difficult cases".into());
    }
    let mut source_ids = BTreeSet::new();
    let mut source_paths = BTreeSet::new();
    for source in &config.sources {
        VaultPath::parse(&source.path)?;
        if !source.logical_id.starts_with('S')
            || !source.path.starts_with("docs/adr/")
            || !source.path.ends_with(".md")
            || !source_ids.insert(source.logical_id.as_str())
            || !source_paths.insert(source.path.as_str())
        {
            return Err(
                "M6 source allowlist must contain unique tracked ADR Markdown files".into(),
            );
        }
    }
    let ids = source_ids.clone();
    let source_splits = config
        .sources
        .iter()
        .map(|source| (source.logical_id.as_str(), &source.split))
        .collect::<BTreeMap<_, _>>();
    let mut task_ids = BTreeSet::new();
    let mut query_refs = BTreeSet::new();
    let mut used_sources = BTreeSet::new();
    for task in &config.tasks {
        if task.id.trim().is_empty()
            || task.query.trim().is_empty()
            || task.source_ids.is_empty()
            || task.source_ids.iter().any(|id| !ids.contains(id.as_str()))
            || task.source_ids.iter().collect::<BTreeSet<_>>().len() != task.source_ids.len()
            || !task_ids.insert(task.id.as_str())
        {
            return Err("M6 task identity or source scope is invalid".into());
        }
        if task.source_ids.iter().any(|source_id| {
            source_splits
                .get(source_id.as_str())
                .is_none_or(|split| *split != &task.split)
        }) {
            return Err("M6 task split must match every referenced source split".into());
        }
        used_sources.extend(task.source_ids.iter().map(String::as_str));
        if task
            .query_refs
            .iter()
            .any(|query_ref| query_ref.trim().is_empty() || !query_refs.insert(query_ref.as_str()))
        {
            return Err("M6 query references must be nonempty and globally unique".into());
        }
        for relation in &task.expected_source_relations {
            if relation.source_ids.is_empty()
                || relation
                    .source_ids
                    .iter()
                    .any(|id| !task.source_ids.contains(id))
            {
                return Err("M6 expected relation exceeds its task source scope".into());
            }
        }
    }
    if used_sources.len() != config.sources.len()
        || config
            .sources
            .iter()
            .any(|source| !used_sources.contains(source.logical_id.as_str()))
    {
        return Err("every M6 source must be covered by its own split's task set".into());
    }
    let b_sources = config.b_source_ids.iter().collect::<BTreeSet<_>>();
    if b_sources.len() != config.b_source_ids.len()
        || b_sources.iter().any(|id| !ids.contains(id.as_str()))
        || config.b_source_ids.len() != 8
    {
        return Err("M6 B arm must select eight distinct in-manifest sources".into());
    }
    for task in config
        .tasks
        .iter()
        .filter(|task| task.split == TaskSplit::Holdout)
    {
        if task
            .source_ids
            .iter()
            .filter(|id| config.b_source_ids.contains(id))
            .count()
            != 1
        {
            return Err("M6 B arm must select exactly one source for every holdout task".into());
        }
    }
    Ok(())
}

fn validate_private_run_root(path: &Path) -> Result<()> {
    if path.components().any(|component| {
        matches!(component, Component::Normal(name) if name == "data" || name == "vault" || name == "production")
    }) {
        return Err("M6 run root overlaps a data, Vault, or production path".into());
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() || fs::canonicalize(path)? != path {
        return Err("M6 run root must be a canonical directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o7777 != 0o700 {
            return Err("M6 run root must have mode 0700".into());
        }
    }
    #[cfg(not(unix))]
    return Err("M6 live evaluation requires verifiable Unix private permissions".into());
    Ok(())
}

async fn create_isolated_fixtures(
    draft: &DraftConfig,
) -> Result<(ArmFixture, ArmFixture, ArmFixture, PathBuf)> {
    let baseline = open_arm(draft, "baseline", "m6-baseline").await?;
    let b_arm = open_arm(draft, "arm-b", "m6-arm-b").await?;
    let c_arm = open_arm(draft, "arm-c", "m6-arm-c").await?;
    let keys_dir = draft.run_root.join("keys");
    create_private_directory(&keys_dir)?;
    let master_key_path = keys_dir.join("master-key");
    Ok((baseline, b_arm, c_arm, master_key_path))
}

async fn open_arm(draft: &DraftConfig, directory: &str, slug: &str) -> Result<ArmFixture> {
    let arm_root = draft.run_root.join(directory);
    create_private_directory(&arm_root)?;
    let source_root = arm_root.join("content");
    let state_root = arm_root.join("state");
    let history_root = arm_root.join("history");
    create_private_directory(&source_root)?;
    create_private_directory(&state_root)?;
    create_private_directory(&history_root)?;
    let state_database = state_root.join("state.sqlite3");
    let state =
        StateStore::connect_and_migrate(&format!("sqlite://{}", state_database.display())).await?;
    set_private_file(&state_database)?;
    let integrity = state.integrity_check().await?;
    if !integrity.integrity_ok
        || integrity.foreign_key_violations != 0
        || integrity.migration_version != 43
    {
        return Err("new M6 State database is not healthy current-schema state".into());
    }
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new(slug)?,
        source_root.clone(),
        Revision::ZERO,
    )?;
    state
        .vaults()
        .insert(&context, slug, VaultStatus::Active)
        .await?;
    state
        .settings()
        .set_vault(
            &context,
            "memory.units.policy",
            &json!({"enabled":true,"request_timeout_seconds":300}),
            WritePrecondition::Unconditional,
            None,
        )
        .await?;
    let core = VaultCore::new(
        state.clone(),
        history_root.clone(),
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
        Default::default(),
    );
    Ok(ArmFixture {
        state,
        context,
        core,
        source_root,
        state_root,
        history_root,
    })
}

async fn read_current_extraction_provider(draft: &DraftConfig) -> Result<FrozenProvider> {
    let state = StateStore::connect_read_only(&format!(
        "sqlite://{}",
        draft.source_database_path.display()
    ))
    .await?;
    let vaults = state.vaults().list().await?;
    let mut selected: Option<(ModelRecord, ProviderRecord, VaultContext, String)> = None;
    for record in vaults {
        if draft
            .source_vault_slug
            .as_deref()
            .is_some_and(|slug| slug != record.slug.as_str())
        {
            continue;
        }
        let context = record.context()?;
        let Some(binding) = state
            .providers()
            .resolve_binding(&context, PROVIDER_ROLE)
            .await?
        else {
            continue;
        };
        let model = state
            .providers()
            .get_model(binding.model_id)
            .await?
            .ok_or("current memory extraction model is missing")?;
        let provider = state
            .providers()
            .get_provider(model.provider_id)
            .await?
            .ok_or("current memory extraction Provider is missing")?;
        match &selected {
            Some((current_model, _, _, _)) if current_model.id != model.id => {
                return Err(
                    "multiple distinct memory extraction bindings require a source Vault slug"
                        .into(),
                );
            }
            Some(_) => continue,
            None => selected = Some((model, provider, context, record.slug.as_str().to_owned())),
        }
    }
    let Some((model, provider, _context, source_vault_slug)) = selected else {
        return Err("no enabled memory_extraction role binding was found".into());
    };
    if !provider.enabled || !model.enabled {
        return Err("current memory extraction Provider or model is disabled".into());
    }
    if !provider.settings["headers"].is_null()
        && provider.settings["headers"]
            .as_object()
            .is_some_and(|headers| !headers.is_empty())
    {
        return Err("M6 credential isolation refuses plaintext custom Provider headers".into());
    }
    let settings: ProviderSettings = serde_json::from_value(provider.settings.clone())?;
    if !settings.headers.is_empty() {
        return Err("M6 credential isolation refuses plaintext custom Provider headers".into());
    }
    let base_url = Url::parse(&provider.base_url)?;
    if !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
    {
        return Err(
            "M6 Provider endpoint must not contain inline credentials or query secrets".into(),
        );
    }
    let Some(secret_id) = provider.secret_id else {
        return Err("current memory extraction Provider secret is missing".into());
    };
    let source_keys = MasterKeyRing::load_file(&draft.source_master_key_path).await?;
    let source_auth = AuthService::new(state.auth(), source_keys);
    let owner_id = provider.id.to_string();
    let secret = source_auth
        .read_installation_secret(
            secret_id,
            PROVIDER_SECRET_PURPOSE,
            PROVIDER_SECRET_OWNER,
            Some(&owner_id),
        )
        .await?;
    Ok(FrozenProvider {
        source_vault_slug,
        source_provider_type: provider.provider_type.clone(),
        source_model_id: model.id.to_string(),
        external_model_id: model.external_model_id,
        provider_kind: ProviderKind::try_from(provider.provider_type.as_str())?,
        base_url,
        settings,
        capabilities: ModelCapabilities::from_json(&model.capabilities)?,
        model_settings: ModelSettings::from_json(&model.settings)?,
        secret,
    })
}

async fn import_sources(
    arm: &ArmFixture,
    sources: &[SourceMaterial],
) -> Result<BTreeMap<String, EvalSource>> {
    let semantic = SemanticMemoryService::new(arm.state.clone());
    let rules_revision = arm
        .state
        .semantic_rules()
        .current_rules_revision(&arm.context)
        .await?;
    let mut manifest_sources = BTreeMap::new();
    for source in sources {
        let path = VaultPath::parse(&source.draft.path)?;
        let created = arm
            .core
            .create_bytes(
                &arm.context,
                &path,
                &source.bytes,
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await?;
        // Preparing locally registers the exact current source and revision;
        // the returned source blocks remain in memory and are never logged.
        let _input = semantic
            .prepare_source(&arm.context, &arm.core, &path)
            .await?;
        let source_record = arm
            .state
            .semantic_memory()
            .get_source_by_file(&arm.context, created.file.id)
            .await?
            .ok_or("imported Markdown source was not registered")?;
        let source_revision_id = source_record
            .current_revision_id
            .ok_or("imported source revision is missing")?;
        let source_revision = arm
            .state
            .semantic_memory()
            .get_source_revision(&arm.context, source_revision_id)
            .await?
            .ok_or("imported source revision record is missing")?;
        let content_hash = format!("{:x}", Sha256::digest(&source.bytes));
        if content_hash != source.content_hash {
            return Err("committed M6 source hash changed during preparation".into());
        }
        if source_record.vault_id != arm.context.id()
            || source_record.file_id != created.file.id
            || source_record.source_path != path
            || source_record.content_hash.trim_start_matches("sha256:") != content_hash
            || source_revision.file_id != created.file.id
            || source_revision.file_revision != created.file.current_revision
            || source_revision.source_path != path
            || source_revision.content_hash.trim_start_matches("sha256:") != content_hash
            || source_record.eligible
            || !source_record.pending_rebuild
            || source_record.source_generation != 0
            || source_record.extraction_commit_sequence != 0
            || source_record.invalid_reason.as_deref() != Some("source_changed")
        {
            return Err("imported source did not satisfy the fresh semantic fence".into());
        }
        manifest_sources.insert(
            source.draft.logical_id.clone(),
            EvalSource {
                logical_id: source.draft.logical_id.clone(),
                synthetic_placeholder: false,
                vault_id: arm.context.id().to_string(),
                file_id: created.file.id.to_string(),
                path: path.to_string(),
                file_revision: created.file.current_revision.value(),
                content_hash,
                logical_block_count: source
                    .bytes
                    .split(|byte| *byte == b'\n')
                    .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
                    .count()
                    .try_into()
                    .map_err(|_| "source block count overflow")?,
                source_revision_id: source_revision_id.to_string(),
                authorization_revision: source_record.authorization_revision,
                profile_id: EXTRACTION_PROFILE.to_owned(),
                rules_revision,
                source_generation: source_record.source_generation,
                extraction_commit_sequence: source_record.extraction_commit_sequence,
            },
        );
    }
    Ok(manifest_sources)
}

fn read_committed_sources(
    repository_root: &Path,
    drafts: &[SourceDraft],
) -> Result<Vec<SourceMaterial>> {
    let mut sources = Vec::with_capacity(drafts.len());
    for draft in drafts {
        let output = Command::new("git")
            .current_dir(repository_root)
            .args(["show", &format!("HEAD:{}", draft.path)])
            .output()?;
        if !output.status.success() || std::str::from_utf8(&output.stdout).is_err() {
            return Err("selected M6 source is not a committed UTF-8 document".into());
        }
        let content_hash = format!("{:x}", Sha256::digest(&output.stdout));
        sources.push(SourceMaterial {
            draft: draft.clone(),
            bytes: output.stdout,
            content_hash,
        });
    }
    Ok(sources)
}

fn git_output(repository_root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(repository_root)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err("could not resolve the committed M6 source revision".into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn arm_root(arm: &ArmFixture) -> SemanticArmRoot {
    SemanticArmRoot {
        vault_slug: arm.context.slug().as_str().to_owned(),
        vault_id: arm.context.id().to_string(),
        source_root: arm.source_root.display().to_string(),
        state_root: arm.state_root.display().to_string(),
        history_root: arm.history_root.display().to_string(),
    }
}

fn source_split(source_id: &str, sources: &[SourceDraft]) -> Option<TaskSplit> {
    sources
        .iter()
        .find(|source| source.logical_id == source_id)
        .map(|source| source.split.clone())
}

fn create_private_directory(path: &Path) -> Result<()> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || fs::canonicalize(path)? != path
        {
            return Err("M6 root component must be a canonical directory".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o7777 != 0o700 {
                return Err("M6 root component must have mode 0700".into());
            }
        }
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        builder.create(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err("M6 private directory modes cannot be established on this platform".into())
    }
}

fn set_private_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("M6 sensitive path must be a regular file".into());
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err("M6 private file modes cannot be established on this platform".into())
    }
}

async fn load_or_create_private_master_key(path: &Path) -> Result<MasterKeyRing> {
    if !tokio::fs::try_exists(path).await? {
        let mut key = Zeroizing::new(vec![0_u8; 32]);
        OsRng.fill_bytes(key.as_mut_slice());
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(path).await {
            Ok(mut file) => {
                file.write_all(key.as_slice()).await?;
                file.sync_all().await?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(std::fs::Permissions::from_mode(0o600))
                        .await?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(MasterKeyRing::load_file(path).await?)
}

fn write_private_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut file = options.open(path)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, bytes);
        Err("M6 private files cannot be written on this platform".into())
    }
}
