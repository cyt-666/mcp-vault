//! Shared construction of an isolated live evaluation runtime.
//!
//! Both the full M6 runner and the source diagnostic use this path so that
//! authentication, state/schema checks, Vault identity checks, secret loading,
//! Provider snapshots, and arm isolation cannot drift between entry points.

use fs2::FileExt;
use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_core::VaultCore;
use mcp_vault_domain::{ModelId, VaultPathPolicy};
use mcp_vault_providers::{ProviderRuntimeSnapshot, ProviderService};
use mcp_vault_state::StateStore;
use mcp_vault_storage_fs::{DurabilityPolicy, StorageOptions};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::private_fs;
use crate::{
    ArmSemanticRuntime, LiveEvaluationConfig, LiveTransportRequestBudget,
    ProviderServiceAppBoundary, ProviderStageTemplate, SemanticArmRoot,
    SemanticMemoryServiceAppBoundary, VaultCoreSourceVerifier, canonical_live_evaluation_hash,
    inspect_live_resume, prepare_live_private_directories, validate_external_model_identity,
    validate_isolated_master_key_path, validate_isolated_state_database_leaf,
    validate_live_evaluation_config,
};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

pub const LIVE_PREPARATION_SEAL: &str = ".live-prepared-seal.json";
const LIVE_PREPARATION_CLAIM: &str = ".live-prepared-claim";
const LIVE_PREPARATION_SEAL_SCHEMA: &str = "semantic-memory-eval-live-prepared-v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LivePreparationSeal {
    schema_version: String,
    evaluation_hash: String,
    paths: Vec<String>,
}

/// The JSON envelope emitted by the preparation binary and consumed by both
/// live entry points.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveCliConfig {
    pub evaluation: LiveEvaluationConfig,
    pub vault_slug: String,
    pub master_key_path: String,
    pub model_id: String,
    pub templates: Vec<ProviderStageTemplate>,
}

/// Fully isolated application boundaries for one prepared live run.
pub struct LiveRuntime {
    pub evaluation: LiveEvaluationConfig,
    pub state_db: PathBuf,
    pub context: mcp_vault_domain::VaultContext,
    pub verifier: VaultCoreSourceVerifier,
    pub provider: ProviderServiceAppBoundary,
    pub semantic: SemanticMemoryServiceAppBoundary,
    _run_lease: fs::File,
}

fn evaluation_hash(evaluation: &LiveEvaluationConfig) -> Result<String> {
    let bytes = serde_json::to_vec(evaluation)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn preparation_paths(evaluation: &LiveEvaluationConfig, master_key_path: &Path) -> Vec<String> {
    [
        evaluation.run_root.as_str(),
        evaluation.source_root.as_str(),
        evaluation.state_root.as_str(),
        evaluation.history_root.as_str(),
        evaluation.artifact_root.as_str(),
        evaluation.semantic_arm_roots.b.source_root.as_str(),
        evaluation.semantic_arm_roots.b.state_root.as_str(),
        evaluation.semantic_arm_roots.b.history_root.as_str(),
        evaluation.semantic_arm_roots.c.source_root.as_str(),
        evaluation.semantic_arm_roots.c.state_root.as_str(),
        evaluation.semantic_arm_roots.c.history_root.as_str(),
        &master_key_path.display().to_string(),
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// Write the immutable seal emitted by the preparation binary. The seal never
/// contains source text, prompts, credentials, or Provider responses.
pub fn write_live_preparation_seal(
    evaluation: &LiveEvaluationConfig,
    master_key_path: &Path,
) -> Result<()> {
    let root = Path::new(&evaluation.run_root);
    let path = root.join(LIVE_PREPARATION_SEAL);
    let seal = LivePreparationSeal {
        schema_version: LIVE_PREPARATION_SEAL_SCHEMA.to_owned(),
        evaluation_hash: evaluation_hash(evaluation)?,
        paths: preparation_paths(evaluation, master_key_path),
    };
    let bytes = serde_json::to_vec_pretty(&seal)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Atomically claim a prepared root before opening or migrating any State DB.
/// A claim is durable and one-shot, so a failed/old/replayed root cannot be
/// repurposed with a new artifact directory.
pub fn claim_live_preparation(
    evaluation: &LiveEvaluationConfig,
    master_key_path: &Path,
) -> Result<fs::File> {
    let root = Path::new(&evaluation.run_root);
    let seal_path = root.join(LIVE_PREPARATION_SEAL);
    private_fs::validate_no_symlink_components(&seal_path)
        .map_err(|_| "prepared live root seal path is unsafe")?;
    private_fs::validate_private_file(&seal_path)
        .map_err(|_| "prepared live root seal must be a private regular file")?;
    let seal_bytes =
        fs::read(&seal_path).map_err(|_| "prepared live root seal is missing or unreadable")?;
    let seal: LivePreparationSeal =
        serde_json::from_slice(&seal_bytes).map_err(|_| "prepared live root seal is invalid")?;
    if seal.schema_version != LIVE_PREPARATION_SEAL_SCHEMA
        || seal.evaluation_hash != evaluation_hash(evaluation)?
        || seal.paths != preparation_paths(evaluation, master_key_path)
    {
        return Err("prepared live root seal does not match the complete config".into());
    }
    let claim_path = root.join(LIVE_PREPARATION_CLAIM);
    private_fs::validate_no_symlink_components(&claim_path)
        .map_err(|_| "prepared live claim path is unsafe")?;
    let claim_existed = claim_path.exists();
    if claim_existed {
        private_fs::validate_private_file(&claim_path)
            .map_err(|_| "prepared live claim must be a private regular file")?;
    }
    let mut claim = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&claim_path)?;
    claim
        .try_lock_exclusive()
        .map_err(|_| "prepared live root already has an active runner")?;
    let marker = format!("{}\n", seal.evaluation_hash);
    let mut existing = String::new();
    claim.read_to_string(&mut existing)?;
    if existing.is_empty() {
        claim.write_all(marker.as_bytes())?;
        claim.sync_all()?;
    } else if existing != marker {
        return Err("prepared live claim belongs to another sealed config".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&claim_path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(claim)
}

/// Construct the same isolated runtime as the complete M6 CLI.
///
/// `request_budget_limit` is used only by the diagnostic entry point to bind
/// the actual Provider transport to two requests. The frozen configuration and
/// its 30/60 manifest/run-config validation remain unchanged.
pub async fn build_live_runtime(
    mut config: LiveCliConfig,
    request_budget_limit: Option<u32>,
) -> Result<LiveRuntime> {
    let mut evaluation = config.evaluation;
    evaluation.provider_model_id = Some(config.model_id.clone());
    evaluation.provider_templates = config.templates.clone();
    let run_root = PathBuf::from(&evaluation.run_root);
    if !run_root.is_absolute()
        || run_root.components().any(|component| {
            matches!(
                component,
                std::path::Component::Normal(value)
                    if value == "data" || value == "vault" || value == "production"
            )
        })
        || [
            &evaluation.state_root,
            &evaluation.history_root,
            &evaluation.artifact_root,
        ]
        .iter()
        .any(|path| !PathBuf::from(path).starts_with(&run_root))
    {
        return Err(
            "run root and state/history/artifact roots must be absolute and isolated".into(),
        );
    }
    let master_key_path = PathBuf::from(&config.master_key_path);
    if !master_key_path.is_absolute()
        || master_key_path.components().any(|component| {
            matches!(
                component,
                std::path::Component::Normal(value)
                    if value == "data" || value == "vault" || value == "production"
            )
        })
    {
        return Err("master key reference must be an absolute isolated path".into());
    }
    evaluation.isolated_master_key_path = Some(master_key_path.display().to_string());
    let (manifest_hash, run_config_hash) = validate_live_evaluation_config(&evaluation)?;
    validate_isolated_master_key_path(&evaluation.run_root, &master_key_path)?;
    let live_config_hash = canonical_live_evaluation_hash(&evaluation)?;
    let run_lease = claim_live_preparation(&evaluation, &master_key_path)?;
    let artifact_root = Path::new(&evaluation.artifact_root);
    let resume = inspect_live_resume(
        &evaluation,
        &manifest_hash,
        &run_config_hash,
        &live_config_hash,
    )?;
    if artifact_root.exists()
        && fs::read_dir(artifact_root)
            .map_err(|_| "isolated artifact root is unreadable")?
            .next()
            .is_some()
        && resume.is_none()
    {
        return Err("isolated artifact root must be empty before runtime construction".into());
    }
    prepare_live_private_directories(&evaluation, &master_key_path)?;
    validate_isolated_master_key_path(&evaluation.run_root, &master_key_path)?;

    let state_db = validate_isolated_state_database_leaf(
        &evaluation.run_root,
        &PathBuf::from(&evaluation.state_root),
    )?;
    let state =
        StateStore::connect_and_migrate(&format!("sqlite://{}", state_db.display())).await?;
    validate_isolated_state_database_leaf(
        &evaluation.run_root,
        &PathBuf::from(&evaluation.state_root),
    )?;
    let integrity = state.integrity_check().await?;
    if !integrity.integrity_ok
        || integrity.foreign_key_violations != 0
        || integrity.migration_version != 43
    {
        return Err("isolated state DB is not a healthy current-schema database".into());
    }
    let slug = mcp_vault_domain::VaultSlug::new(&config.vault_slug)?;
    let record = state
        .vaults()
        .find_by_slug(&slug)
        .await?
        .ok_or("isolated Vault is not registered in the current-schema DB")?;
    let context = record.context()?;
    if context.content_root() != std::path::Path::new(&evaluation.source_root)
        || context.id().to_string() != evaluation.isolated_vault_id
    {
        return Err("isolated Vault context does not match evaluation roots/identity".into());
    }
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::load_file(&master_key_path).await?,
    );
    let providers = ProviderService::new(state.clone(), auth);
    let model_id = ModelId::parse(&config.model_id)?;
    let model = state
        .providers()
        .get_model(model_id)
        .await?
        .ok_or("registered Provider model is missing")?;
    let provider_runtime: ProviderRuntimeSnapshot =
        providers.runtime_snapshot(&context, model_id).await?;
    evaluation.provider_runtime_snapshot = Some(provider_runtime.clone());
    let budget_limit = request_budget_limit.unwrap_or(
        evaluation.run_config.comparisons[0]
            .budget
            .external_request_budget,
    );
    let transport_budget = LiveTransportRequestBudget::new_with_used(
        budget_limit,
        resume
            .as_ref()
            .map_or(0, |resume| resume.provider_requests_consumed),
    )?;
    let providers = providers.with_generation_budget(Arc::new(transport_budget.clone()));
    validate_external_model_identity(
        &evaluation.run_config,
        &config.templates,
        &model.external_model_id,
    )?;
    let provider = ProviderServiceAppBoundary::new(
        providers,
        context.clone(),
        model_id,
        &model.external_model_id,
        std::mem::take(&mut config.templates),
        provider_runtime,
        transport_budget,
    )?;
    let core = VaultCore::new(
        state.clone(),
        PathBuf::from(&evaluation.history_root),
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
        Default::default(),
    );
    let verifier =
        VaultCoreSourceVerifier::load(&state, &context, &core, &evaluation.manifest).await?;
    let b_runtime =
        open_arm_runtime(&evaluation.run_root, &evaluation.semantic_arm_roots.b).await?;
    let c_runtime =
        open_arm_runtime(&evaluation.run_root, &evaluation.semantic_arm_roots.c).await?;
    let semantic = SemanticMemoryServiceAppBoundary::new_isolated(
        state.clone(),
        context.clone(),
        state_db.clone(),
        PathBuf::from(&evaluation.history_root),
        b_runtime,
        c_runtime,
    )?;
    let provider = provider.with_prepared_extraction_cleanup(semantic.clone());
    Ok(LiveRuntime {
        evaluation,
        state_db,
        context,
        verifier,
        provider,
        semantic,
        _run_lease: run_lease,
    })
}

async fn open_arm_runtime(run_root: &str, config: &SemanticArmRoot) -> Result<ArmSemanticRuntime> {
    let state_db_path =
        validate_isolated_state_database_leaf(run_root, &PathBuf::from(&config.state_root))?;
    let state =
        StateStore::connect_and_migrate(&format!("sqlite://{}", state_db_path.display())).await?;
    validate_isolated_state_database_leaf(run_root, &PathBuf::from(&config.state_root))?;
    let integrity = state.integrity_check().await?;
    if !integrity.integrity_ok
        || integrity.foreign_key_violations != 0
        || integrity.migration_version != 43
    {
        return Err("isolated semantic arm DB is not healthy current-schema state".into());
    }
    let slug = mcp_vault_domain::VaultSlug::new(&config.vault_slug)?;
    let record = state
        .vaults()
        .find_by_slug(&slug)
        .await?
        .ok_or("isolated semantic arm Vault is not registered")?;
    let context = record.context()?;
    if context.content_root() != std::path::Path::new(&config.source_root)
        || context.id().to_string() != config.vault_id
    {
        return Err("semantic arm Vault context does not match its isolated roots".into());
    }
    let source_root = PathBuf::from(&config.source_root);
    let history_root = PathBuf::from(&config.history_root);
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
    Ok(ArmSemanticRuntime {
        state,
        context,
        core,
        source_root,
        state_db_path,
        history_root,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EvalMode, EvaluationManifest, EvaluationRunConfig, LIVE_RUN_SCHEMA, SemanticArmRoots,
    };

    fn test_evaluation(root: &Path) -> LiveEvaluationConfig {
        let path = |name: &str| root.join(name).display().to_string();
        LiveEvaluationConfig {
            schema_version: LIVE_RUN_SCHEMA.into(),
            semantic_protocol: String::new(),
            manifest: EvaluationManifest {
                schema_version: "semantic-memory-eval-manifest-v1".into(),
                dataset_id: "seal-test".into(),
                synthetic_only: false,
                sources: vec![],
                tasks: vec![],
            },
            run_config: EvaluationRunConfig {
                schema_version: "semantic-memory-eval-run-config-v1".into(),
                mode: EvalMode::Live,
                allow_live: true,
                explicit_live_authorization: true,
                source_allowlist: vec![],
                cost_budget_minor: 0,
                artifact_root: Some(path("artifacts")),
                comparisons: vec![],
            },
            run_root: root.display().to_string(),
            source_root: path("source"),
            state_root: path("state"),
            history_root: path("history"),
            artifact_root: path("artifacts"),
            isolated_vault_id: "vault".into(),
            semantic_arm_roots: SemanticArmRoots {
                b: SemanticArmRoot {
                    vault_slug: "b".into(),
                    vault_id: "b-vault".into(),
                    source_root: path("b/source"),
                    state_root: path("b/state"),
                    history_root: path("b/history"),
                },
                c: SemanticArmRoot {
                    vault_slug: "c".into(),
                    vault_id: "c-vault".into(),
                    source_root: path("c/source"),
                    state_root: path("c/state"),
                    history_root: path("c/history"),
                },
            },
            current_schema: LIVE_RUN_SCHEMA.into(),
            task_budget: 0,
            unbounded_cost_authorized: true,
            provider_model_id: None,
            provider_runtime_snapshot: None,
            provider_templates: vec![],
            isolated_master_key_path: None,
        }
    }

    #[test]
    fn preparation_seal_is_path_bound_and_lease_is_exclusive() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let mut evaluation = test_evaluation(&root);
        let key = root.join("master.key");
        write_live_preparation_seal(&evaluation, &key).unwrap();
        let lease = claim_live_preparation(&evaluation, &key).unwrap();
        assert!(claim_live_preparation(&evaluation, &key).is_err());
        drop(lease);
        let resumed_lease = claim_live_preparation(&evaluation, &key).unwrap();
        drop(resumed_lease);

        evaluation.artifact_root = root.join("other-artifacts").display().to_string();
        assert!(claim_live_preparation(&evaluation, &key).is_err());
    }

    #[test]
    fn a80_resume_counts_every_started_attempt_and_fences_hashes() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let mut evaluation = test_evaluation(&root);
        evaluation.semantic_protocol = "m1-a80-v1".into();
        evaluation.artifact_root = root.join("artifacts").display().to_string();
        evaluation.manifest.sources.push(crate::EvalSource {
            logical_id: "S01".into(),
            synthetic_placeholder: false,
            vault_id: "vault".into(),
            file_id: "file".into(),
            path: "notes/source.md".into(),
            file_revision: 1,
            content_hash: "a".repeat(64),
            logical_block_count: 1,
            source_revision_id: "revision".into(),
            authorization_revision: 1,
            profile_id: "profile".into(),
            rules_revision: 1,
            source_generation: 1,
            extraction_commit_sequence: 1,
        });
        let artifacts = Path::new(&evaluation.artifact_root);
        private_fs::ensure_private_directory(artifacts).unwrap();
        let checkpoint = serde_json::json!({
            "schema_version":LIVE_RUN_SCHEMA,
            "manifest_hash":"manifest-hash",
            "run_config_hash":"run-config-hash",
            "live_config_hash":"live-config-hash",
            "last_boundary":"observation_batch",
            "provider_requests":0,
            "completed_tasks":0,
            "status":"running",
            "first_error":null
        });
        private_fs::atomic_write_private_file(
            &artifacts.join("checkpoint.json"),
            &serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let attempt = serde_json::json!({
            "sequence":1,
            "stage":"observation",
            "arm":"b",
            "source_id":"S01",
            "task_id":null,
            "batch_index":0,
            "status":"started"
        });
        private_fs::atomic_write_private_file(
            &artifacts.join("attempts.jsonl"),
            format!("{}\n", attempt).as_bytes(),
        )
        .unwrap();
        let resume = crate::inspect_live_resume(
            &evaluation,
            "manifest-hash",
            "run-config-hash",
            "live-config-hash",
        )
        .unwrap()
        .unwrap();
        assert_eq!(resume.provider_requests_consumed, 1);
        assert_eq!(resume.last_attempt_sequence, 1);
        assert_eq!(resume.attempts.len(), 1);
        assert!(
            crate::inspect_live_resume(
                &evaluation,
                "different-manifest",
                "run-config-hash",
                "live-config-hash",
            )
            .is_err()
        );
    }

    #[test]
    fn missing_preparation_seal_rejects_before_claim() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let evaluation = test_evaluation(&root);
        let key = root.join("master.key");
        assert!(claim_live_preparation(&evaluation, &key).is_err());
        assert!(!root.join(LIVE_PREPARATION_CLAIM).exists());
    }
}
