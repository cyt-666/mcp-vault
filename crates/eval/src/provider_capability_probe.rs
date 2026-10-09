//! Explicitly authorized, two-request Provider capability probe.
//!
//! This is intentionally smaller than the M6 quality evaluator.  It validates
//! only that a configured structured-generation model and embedding model can
//! be reached with one fixed synthetic input each.  The production boundary is
//! built from the existing Provider/Auth/State services; tests use the narrow
//! [`CapabilityProbeProvider`] seam and never need a network.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};

#[cfg(unix)]
use std::{fs::OpenOptions, io::Write};

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use mcp_vault_auth::{AuthService, MasterKeyRing, SecretString};
use mcp_vault_core::ManagedVaultService;
use mcp_vault_domain::{ProviderId, VaultContext, VaultSlug};
use mcp_vault_providers::{
    EmbeddingRequest, ModelCapabilities, ModelInput, ModelSettings, OpenAiThinkingMode,
    PROVIDER_SECRET_OWNER, PROVIDER_SECRET_PURPOSE, ProviderError, ProviderInput, ProviderKind,
    ProviderMode, ProviderService, ProviderSettings, RequestBudget, StructuredGenerationRequest,
    StructuredGenerationResult,
};
use mcp_vault_state::StateStore;
use mcp_vault_storage_fs::{DurabilityPolicy, StorageOptions};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use thiserror::Error;
use tokio::fs as async_fs;
use url::Url;

use super::private_fs;

#[cfg(unix)]
use rand::{RngCore, rngs::OsRng};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use zeroize::Zeroizing;

/// Current configuration/checkpoint schema.  This is independent from the
/// broader semantic-card live-evaluation schema.
pub const PROVIDER_CAPABILITY_PROBE_SCHEMA: &str = "provider-capability-probe-v2";
/// The highest migration shipped by this checkout.
pub const CURRENT_STATE_MIGRATION: i64 = 43;
/// Fixed input used by both probe stages.  It contains no Vault or user text.
pub const SYNTHETIC_PROBE_INPUT: &str = "mcp-vault provider capability probe synthetic input v1";
const PREPARED_SEAL_SCHEMA: &str = "provider-capability-probe-prepared-v1";
const CHECKPOINT_FILE: &str = "provider-capability-probe.checkpoint.json";
const PREPARED_SEAL_FILE: &str = ".provider-capability-probe.prepared";
const PROBE_GENERATION_TOKEN_LIMIT: u32 = 256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PreparedStateSeal {
    schema_version: String,
    config_tag: String,
    vault_id: String,
    generation_provider_id: String,
    embedding_provider_id: String,
    generation_model_id: String,
    embedding_model_id: String,
    seal_tag: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PreparedStateIdentity {
    vault_id: String,
    generation_provider_id: String,
    embedding_provider_id: String,
    generation_model_id: String,
    embedding_model_id: String,
}

#[derive(Serialize)]
struct PreparedStateSealPayload<'a> {
    schema_version: &'a str,
    config_tag: &'a str,
    vault_id: &'a str,
    generation_provider_id: &'a str,
    embedding_provider_id: &'a str,
    generation_model_id: &'a str,
    embedding_model_id: &'a str,
}

fn generation_probe_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {"ok": {"type": "boolean", "enum": [true]}},
        "required": ["ok"],
        "additionalProperties": false
    })
}

fn generation_probe_model_settings() -> ModelSettings {
    ModelSettings {
        openai_thinking_mode: OpenAiThinkingMode::Disabled,
        generation_token_limit: Some(PROBE_GENERATION_TOKEN_LIMIT),
        ..ModelSettings::default()
    }
}

/// A provider/model declaration required by the probe.  The endpoint, kind,
/// model ID, and capability metadata are all explicit; no model discovery is
/// performed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeProviderConfig {
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub model_id: String,
    pub capabilities: ModelCapabilities,
}

/// Explicit operator policy for checking or discovering an embedding model's
/// output dimension. A discovered value is evidence from this probe only; it
/// is never copied into the registered model capabilities.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpectedEmbeddingDimension {
    Known { dimension: u32 },
    Discover,
}

/// Explicit source-to-destination secret provisioning. The source DB is opened
/// read-only and the source key decrypts only in memory. The source Provider is
/// an installation-scoped object selected by ID; the destination
/// ProviderService encrypts a new record with its new key.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeSourceCredential {
    pub source_database_path: String,
    pub source_master_key_path: String,
    pub source_provider_id: String,
}

/// Per-role opt-in provisioning; credentials are never implicitly shared
/// between generation and embedding providers.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeCredentialProvisioning {
    #[serde(default)]
    pub generation: Option<ProbeSourceCredential>,
    #[serde(default)]
    pub embedding: Option<ProbeSourceCredential>,
}

/// Complete probe configuration.  Every root and identity is explicit; there
/// is no default `./data`, Vault, production, or key path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCapabilityProbeConfig {
    pub schema_version: String,
    pub run_root: String,
    pub state_root: String,
    pub vault_root: String,
    pub history_root: String,
    pub artifact_root: String,
    pub master_key_path: String,
    pub vault_slug: String,
    pub provider_mode: ProviderMode,
    pub generation: ProbeProviderConfig,
    pub embedding: ProbeProviderConfig,
    pub embedding_expected_dimension: ExpectedEmbeddingDimension,
    #[serde(default)]
    pub credential_provisioning: Option<ProbeCredentialProvisioning>,
}

/// Safe stage names persisted in the checkpoint.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStage {
    Preflight,
    Provisioning,
    Generation,
    Embedding,
    Complete,
}

/// Safe checkpoint status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    Running,
    Succeeded,
    Failed,
}

/// Meaning of the safe dimension value recorded in a probe checkpoint.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeDimensionStatus {
    Matched,
    Discovered,
    Mismatch,
    Invalid,
}

/// Atomic checkpoint payload.  `http_status` is the only HTTP diagnostic kept;
/// endpoint URLs, headers, credentials, prompts, outputs, response bodies,
/// vectors, and source text are deliberately excluded.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeCheckpoint {
    pub stage: ProbeStage,
    pub status: ProbeStatus,
    pub generation_provider_kind: ProviderKind,
    pub embedding_provider_kind: ProviderKind,
    pub generation_model_id: String,
    pub embedding_model_id: String,
    pub request_count: u32,
    pub error_code: Option<String>,
    pub http_status: Option<u16>,
    pub usage_status: String,
    pub dimension: Option<u32>,
    #[serde(default)]
    pub dimension_status: Option<ProbeDimensionStatus>,
}

/// Errors are intentionally code-only at the Provider boundary.  No wrapped
/// error text is exposed by the CLI or checkpoint.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProbeError {
    #[error("probe configuration is invalid")]
    Config,
    #[error("probe roots are unsafe or overlap")]
    UnsafeRoots,
    #[error("probe state is unavailable")]
    State,
    #[error("probe credential is unavailable")]
    Credential,
    #[error("source credential state could not be opened")]
    SourceStateOpenFailed,
    #[error("source installation key could not be loaded")]
    SourceMasterKeyLoadFailed,
    #[error("source Provider secret reference is unavailable")]
    SourceProviderReferenceUnavailable,
    #[error("source Provider reference schema is incompatible")]
    SourceProviderSchemaIncompatible,
    #[error("source credential reference is invalid")]
    SourceSecretReferenceInvalid,
    #[error("source credential metadata is invalid")]
    SourceSecretMetadataInvalid,
    #[error("source credential could not be decrypted")]
    SourceSecretDecryptFailed,
    #[error("probe checkpoint is unavailable")]
    Checkpoint,
    #[error("probe Provider request failed")]
    Provider(String),
    #[error("probe request budget exhausted")]
    BudgetExhausted,
}

impl ProbeError {
    fn safe_code(&self) -> &str {
        match self {
            Self::Config => "probe_config_invalid",
            Self::UnsafeRoots => "probe_isolation_failed",
            Self::State => "probe_state_error",
            Self::Credential => "probe_credential_error",
            Self::SourceStateOpenFailed => "source_state_open_failed",
            Self::SourceMasterKeyLoadFailed => "source_master_key_load_failed",
            Self::SourceProviderReferenceUnavailable => "source_provider_reference_unavailable",
            Self::SourceProviderSchemaIncompatible => "source_provider_schema_incompatible",
            Self::SourceSecretReferenceInvalid => "source_secret_reference_invalid",
            Self::SourceSecretMetadataInvalid => "source_secret_metadata_invalid",
            Self::SourceSecretDecryptFailed => "source_secret_decrypt_failed",
            Self::Checkpoint => "probe_checkpoint_error",
            Self::Provider(code) => code,
            Self::BudgetExhausted => "probe_request_budget_exhausted",
        }
    }
}

/// Shared atomic budget.  Transport reserves one slot per actual attempt;
/// with retries forced to zero, the successful run can consume at most two.
#[derive(Debug)]
pub struct ProbeRequestBudget {
    limit: u32,
    used: AtomicU32,
}

impl ProbeRequestBudget {
    pub const fn new() -> Self {
        Self {
            limit: 2,
            used: AtomicU32::new(0),
        }
    }

    pub fn used(&self) -> u32 {
        self.used.load(Ordering::SeqCst)
    }
}

impl Default for ProbeRequestBudget {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RequestBudget for ProbeRequestBudget {
    async fn reserve(&self, _body_bytes: usize) -> Result<(), ProviderError> {
        self.used
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                used.checked_add(1).filter(|next| *next <= self.limit)
            })
            .map(|_| ())
            .map_err(|_| ProviderError::InvalidConfiguration("probe_request_budget_exhausted"))
    }
}

/// The narrow application boundary used by the probe runner.
#[async_trait]
pub trait CapabilityProbeProvider: Send + Sync {
    async fn generate(&self) -> Result<StructuredGenerationResult, ProviderError>;
    async fn embed(&self) -> Result<u32, ProviderError>;
}

/// Existing application services wired into the probe boundary.  It performs
/// exactly one explicit generation and one explicit embedding call and never
/// writes a vector projection.
pub struct ProviderServiceCapabilityBoundary {
    service: ProviderService,
    context: VaultContext,
    generation_model: mcp_vault_domain::ModelId,
    embedding_model: mcp_vault_domain::ModelId,
    generation_external_id: String,
    embedding_external_id: String,
    budget: Arc<ProbeRequestBudget>,
}

impl ProviderServiceCapabilityBoundary {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        service: ProviderService,
        context: VaultContext,
        generation_model: mcp_vault_domain::ModelId,
        embedding_model: mcp_vault_domain::ModelId,
        generation_external_id: String,
        embedding_external_id: String,
        budget: Arc<ProbeRequestBudget>,
    ) -> Self {
        Self {
            service,
            context,
            generation_model,
            embedding_model,
            generation_external_id,
            embedding_external_id,
            budget,
        }
    }
}

#[async_trait]
impl CapabilityProbeProvider for ProviderServiceCapabilityBoundary {
    async fn generate(&self) -> Result<StructuredGenerationResult, ProviderError> {
        self.service
            .generate_structured(
                &self.context,
                self.generation_model,
                &StructuredGenerationRequest {
                    model: self.generation_external_id.clone(),
                    system: "Return exactly this one object: {\"ok\":true}.".to_owned(),
                    user: SYNTHETIC_PROBE_INPUT.to_owned(),
                    schema_name: "provider_capability_probe".to_owned(),
                    strict_function_schema: None,
                    defer_local_schema_validation: false,
                    strict_function_call: false,
                    non_stream_json_object: false,
                    schema: generation_probe_schema(),
                    allow_additional_output_properties: false,
                    missing_required_string_fallbacks: Vec::new(),
                    max_output_tokens: PROBE_GENERATION_TOKEN_LIMIT,
                    temperature: Some(0.0),
                    timeout: None,
                },
            )
            .await
    }

    async fn embed(&self) -> Result<u32, ProviderError> {
        let result = self
            .service
            .embed_with_budget(
                &self.context,
                self.embedding_model,
                &EmbeddingRequest {
                    model: self.embedding_external_id.clone(),
                    inputs: vec![SYNTHETIC_PROBE_INPUT.to_owned()],
                },
                self.budget.clone(),
            )
            .await?;
        result
            .vectors
            .first()
            .map(|vector| vector.len() as u32)
            .ok_or(ProviderError::InvalidResponse("embedding result is empty"))
    }
}

/// Validate a new, empty destination before opening State or loading any key.
/// This is the non-mutating preflight contract used by the CLI.
pub fn validate_provider_capability_probe_config(
    config: &ProviderCapabilityProbeConfig,
) -> Result<(), ProbeError> {
    validate_probe_config_fields(config)?;
    validate_source_credentials(config, true)?;

    let run = Path::new(&config.run_root);
    let roots = [
        Path::new(&config.state_root),
        Path::new(&config.vault_root),
        Path::new(&config.history_root),
        Path::new(&config.artifact_root),
    ];
    let key = Path::new(&config.master_key_path);
    let key_parent = key.parent().ok_or(ProbeError::UnsafeRoots)?;

    validate_existing_private_directory(run, 0o700)?;
    if lstat_if_exists(run)?.is_some() {
        ensure_directory_empty(run)?;
    }
    for root in roots {
        validate_existing_private_directory(root, 0o700)?;
        if lstat_if_exists(root)?.is_some() {
            ensure_directory_empty(root)?;
        }
    }
    validate_existing_private_directory(key_parent, 0o700)?;
    if lstat_if_exists(key_parent)?.is_some() {
        ensure_directory_empty(key_parent)?;
    }
    if lstat_if_exists(key)?.is_some() {
        return Err(ProbeError::UnsafeRoots);
    }
    let state_db = Path::new(&config.state_root).join("state.sqlite3");
    if lstat_if_exists(&state_db)?.is_some() {
        return Err(ProbeError::UnsafeRoots);
    }
    Ok(())
}

fn validate_probe_config_fields(config: &ProviderCapabilityProbeConfig) -> Result<(), ProbeError> {
    validate_private_filesystem_support()?;
    if config.schema_version != PROVIDER_CAPABILITY_PROBE_SCHEMA
        || config.run_root.trim().is_empty()
        || config.state_root.trim().is_empty()
        || config.vault_root.trim().is_empty()
        || config.history_root.trim().is_empty()
        || config.artifact_root.trim().is_empty()
        || config.master_key_path.trim().is_empty()
        || config.provider_mode == ProviderMode::Disabled
    {
        return Err(ProbeError::Config);
    }
    let slug = VaultSlug::new(&config.vault_slug).map_err(|_| ProbeError::Config)?;
    if slug.as_str() == "production" || slug.as_str() == "vault" || slug.as_str() == "data" {
        return Err(ProbeError::Config);
    }
    validate_provider_config(&config.generation, true)?;
    validate_provider_config(&config.embedding, false)?;
    if config
        .generation
        .capabilities
        .max_output_tokens
        .is_some_and(|limit| limit < PROBE_GENERATION_TOKEN_LIMIT)
    {
        return Err(ProbeError::Config);
    }
    match config.embedding_expected_dimension {
        ExpectedEmbeddingDimension::Known { dimension }
            if dimension > 0
                && dimension <= 1_000_000
                && config.embedding.capabilities.dimension == Some(dimension) => {}
        ExpectedEmbeddingDimension::Discover
            if config.embedding.capabilities.dimension.is_none() => {}
        _ => return Err(ProbeError::Config),
    }
    let run = Path::new(&config.run_root);
    let roots = [
        Path::new(&config.state_root),
        Path::new(&config.vault_root),
        Path::new(&config.history_root),
        Path::new(&config.artifact_root),
    ];
    let key = Path::new(&config.master_key_path);
    validate_root_set(run, &roots, key)?;
    let expected_vault = run.join("vaults").join(slug.as_str());
    if canonical_candidate(Path::new(&config.vault_root))? != canonical_candidate(&expected_vault)?
    {
        return Err(ProbeError::UnsafeRoots);
    }
    validate_source_credentials(config, false)?;
    Ok(())
}

fn validate_source_credentials(
    config: &ProviderCapabilityProbeConfig,
    inspect_filesystem: bool,
) -> Result<(), ProbeError> {
    if let Some(sources) = &config.credential_provisioning {
        for source in [&sources.generation, &sources.embedding]
            .into_iter()
            .flatten()
        {
            if source.source_database_path.trim().is_empty()
                || source.source_master_key_path.trim().is_empty()
                || source.source_provider_id.trim().is_empty()
                || !Path::new(&source.source_database_path).is_absolute()
                || !Path::new(&source.source_master_key_path).is_absolute()
            {
                return Err(ProbeError::Config);
            }
            mcp_vault_domain::ProviderId::parse(&source.source_provider_id)
                .map_err(|_| ProbeError::Config)?;
            if inspect_filesystem {
                let run = Path::new(&config.run_root);
                let source_db = Path::new(&source.source_database_path);
                let source_key = Path::new(&source.source_master_key_path);
                let source_key_parent = source_key.parent().ok_or(ProbeError::Credential)?;
                validate_private_file(source_key)?;
                validate_private_directory(source_key_parent, 0o500)?;
                if !source_db.is_file() {
                    return Err(ProbeError::Credential);
                }
                if paths_overlap(&canonical_candidate(source_db)?, &canonical_candidate(run)?)
                    || paths_overlap(
                        &canonical_candidate(source_key)?,
                        &canonical_candidate(run)?,
                    )
                {
                    return Err(ProbeError::UnsafeRoots);
                }
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn validate_private_filesystem_support() -> Result<(), ProbeError> {
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_filesystem_support() -> Result<(), ProbeError> {
    Err(ProbeError::UnsafeRoots)
}

fn validate_provider_config(
    config: &ProbeProviderConfig,
    generation: bool,
) -> Result<(), ProbeError> {
    if config.name.trim().is_empty() || config.model_id.trim().is_empty() {
        return Err(ProbeError::Config);
    }
    let url = Url::parse(&config.base_url).map_err(|_| ProbeError::Config)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.cannot_be_a_base()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ProbeError::Config);
    }
    if generation {
        if !config.capabilities.structured_output
            || config.capabilities.embeddings
            || matches!(
                config.kind,
                ProviderKind::EmbeddingHttp | ProviderKind::FastEmbedLocal
            )
        {
            return Err(ProbeError::Config);
        }
    } else if !config.capabilities.embeddings
        || matches!(
            config.kind,
            ProviderKind::OpenAiResponses
                | ProviderKind::AnthropicMessages
                | ProviderKind::FastEmbedLocal
        )
    {
        return Err(ProbeError::Config);
    }
    config
        .capabilities
        .validate()
        .map_err(|_| ProbeError::Config)
}

fn validate_root_set(run: &Path, roots: &[&Path], key: &Path) -> Result<(), ProbeError> {
    if !run.is_absolute()
        || run.components().any(is_forbidden_component)
        || !key.is_absolute()
        || key.components().any(is_forbidden_component)
        || roots
            .iter()
            .any(|path| !path.is_absolute() || path.components().any(is_forbidden_component))
        || std::iter::once(run)
            .chain(roots.iter().copied())
            .chain(std::iter::once(key))
            .any(|path| {
                path.components()
                    .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
            })
    {
        return Err(ProbeError::UnsafeRoots);
    }
    let run_real = canonical_candidate(run)?;
    if roots.iter().any(|path| {
        canonical_candidate(path).is_err_and(|_| true)
            || !canonical_candidate(path)
                .is_ok_and(|real| real.starts_with(&run_real) && real != run_real)
    }) {
        return Err(ProbeError::UnsafeRoots);
    }
    let canonical_roots = roots
        .iter()
        .map(|path| canonical_candidate(path))
        .collect::<Result<Vec<_>, _>>()?;
    for (index, current) in canonical_roots.iter().enumerate() {
        if canonical_roots
            .iter()
            .skip(index + 1)
            .any(|other| paths_overlap(current, other))
        {
            return Err(ProbeError::UnsafeRoots);
        }
    }
    let key_real = canonical_candidate(key)?;
    if canonical_roots
        .iter()
        .any(|root| paths_overlap(root, &key_real))
        || !key_real.starts_with(&run_real)
        || key_real == run_real
    {
        return Err(ProbeError::UnsafeRoots);
    }
    Ok(())
}

fn is_forbidden_component(component: Component<'_>) -> bool {
    matches!(component, Component::Normal(value) if matches!(value.to_str(), Some("data" | "vault" | "production")))
}

fn lstat_if_exists(path: &Path) -> Result<Option<fs::Metadata>, ProbeError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ProbeError::UnsafeRoots),
    }
}

fn validate_existing_private_directory(
    path: &Path,
    required_owner_bits: u32,
) -> Result<(), ProbeError> {
    if lstat_if_exists(path)?.is_some() {
        validate_private_directory(path, required_owner_bits)?;
    }
    Ok(())
}

fn validate_private_directory(path: &Path, required_owner_bits: u32) -> Result<(), ProbeError> {
    if required_owner_bits == 0o700 {
        return private_fs::validate_private_directory(path).map_err(|_| ProbeError::UnsafeRoots);
    }
    let metadata = validate_directory_identity(path)?;
    #[cfg(unix)]
    {
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 || mode & required_owner_bits != required_owner_bits {
            return Err(ProbeError::UnsafeRoots);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = required_owner_bits;
        Err(ProbeError::UnsafeRoots)
    }
}

fn validate_directory_identity(path: &Path) -> Result<fs::Metadata, ProbeError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ProbeError::UnsafeRoots)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || fs::canonicalize(path).map_err(|_| ProbeError::UnsafeRoots)? != path
    {
        return Err(ProbeError::UnsafeRoots);
    }
    Ok(metadata)
}

fn create_private_directory(path: &Path) -> Result<(), ProbeError> {
    private_fs::ensure_private_directory(path).map_err(|_| ProbeError::UnsafeRoots)
}

#[cfg(unix)]
fn create_probe_master_key(
    path: &Path,
) -> Result<(MasterKeyRing, zeroize::Zeroizing<[u8; 32]>), ProbeError> {
    if lstat_if_exists(path)?.is_some() {
        return Err(ProbeError::UnsafeRoots);
    }
    let parent = path.parent().ok_or(ProbeError::UnsafeRoots)?;
    validate_private_directory(parent, 0o700)?;

    let mut key = Zeroizing::new([0_u8; 32]);
    let mut rng = OsRng;
    rng.fill_bytes(&mut key[..]);

    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(path).map_err(|_| ProbeError::Credential)?;
    file.write_all(&key[..])
        .map_err(|_| ProbeError::Credential)?;
    file.sync_all().map_err(|_| ProbeError::Credential)?;
    drop(file);
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|_| ProbeError::Credential)?;
    validate_private_file(path)?;
    let ring = MasterKeyRing::from_bytes(1, &key[..]).map_err(|_| ProbeError::Credential)?;
    Ok((ring, key))
}

#[cfg(not(unix))]
fn create_probe_master_key(
    _path: &Path,
) -> Result<(MasterKeyRing, zeroize::Zeroizing<[u8; 32]>), ProbeError> {
    Err(ProbeError::UnsafeRoots)
}

fn validate_private_file(path: &Path) -> Result<(), ProbeError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ProbeError::UnsafeRoots)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || fs::canonicalize(path).map_err(|_| ProbeError::UnsafeRoots)? != path
    {
        return Err(ProbeError::UnsafeRoots);
    }
    #[cfg(unix)]
    {
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 || mode & 0o400 == 0 {
            return Err(ProbeError::UnsafeRoots);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        Err(ProbeError::UnsafeRoots)
    }
}

fn canonical_candidate(path: &Path) -> Result<PathBuf, ProbeError> {
    let mut suffix = Vec::new();
    let mut ancestor = path;
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(ProbeError::UnsafeRoots);
                }
                let mut candidate =
                    fs::canonicalize(ancestor).map_err(|_| ProbeError::UnsafeRoots)?;
                for component in suffix.iter().rev() {
                    candidate.push(component);
                }
                if candidate != path {
                    return Err(ProbeError::UnsafeRoots);
                }
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(parent) = ancestor.parent() else {
                    return Err(ProbeError::UnsafeRoots);
                };
                if let Some(name) = ancestor.file_name() {
                    suffix.push(name.to_owned());
                }
                ancestor = parent;
            }
            Err(_) => return Err(ProbeError::UnsafeRoots),
        }
    }
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn ensure_directory_empty(path: &Path) -> Result<(), ProbeError> {
    validate_private_directory(path, 0o700)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| ProbeError::UnsafeRoots)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ProbeError::UnsafeRoots);
    }
    let mut entries = fs::read_dir(path).map_err(|_| ProbeError::UnsafeRoots)?;
    if entries
        .next()
        .transpose()
        .map_err(|_| ProbeError::UnsafeRoots)?
        .is_some()
    {
        return Err(ProbeError::UnsafeRoots);
    }
    Ok(())
}

/// Run the two-stage probe against an injected Provider boundary.  The runner
/// updates the checkpoint after every stage and never invokes embedding after
/// generation fails.
pub async fn run_capability_probe<P: CapabilityProbeProvider>(
    config: &ProviderCapabilityProbeConfig,
    provider: &P,
    budget: Arc<ProbeRequestBudget>,
) -> Result<ProbeCheckpoint, ProbeError> {
    let mut checkpoint = match validate_prepared_probe_run(config).await {
        Ok(checkpoint) => checkpoint,
        Err(error) => {
            if record_prepared_validation_failure(config, &error)
                .await
                .is_err()
            {
                return Err(ProbeError::Checkpoint);
            }
            return Err(error);
        }
    };
    let checkpoint_path = checkpoint_path(config);

    checkpoint.stage = ProbeStage::Generation;
    write_checkpoint(&checkpoint_path, &checkpoint).await?;
    let generation = match provider.generate().await {
        Ok(result) => result,
        Err(error) => {
            checkpoint.status = ProbeStatus::Failed;
            record_provider_error(&mut checkpoint, &error);
            checkpoint.request_count = budget.used();
            checkpoint.usage_status = "unavailable".to_owned();
            write_checkpoint(&checkpoint_path, &checkpoint).await?;
            return Err(ProbeError::Provider(error.code().to_owned()));
        }
    };
    checkpoint.request_count = budget.used();
    checkpoint.usage_status = if generation.usage.is_some() {
        "reported"
    } else {
        "unavailable"
    }
    .to_owned();
    if checkpoint.request_count > 2 {
        return Err(ProbeError::BudgetExhausted);
    }
    if generation.value != json!({"ok": true}) {
        checkpoint.status = ProbeStatus::Failed;
        checkpoint.error_code = Some("probe_structured_output_mismatch".to_owned());
        write_checkpoint(&checkpoint_path, &checkpoint).await?;
        return Err(ProbeError::Provider(
            "probe_structured_output_mismatch".to_owned(),
        ));
    }
    checkpoint.stage = ProbeStage::Embedding;
    write_checkpoint(&checkpoint_path, &checkpoint).await?;
    let dimension = match provider.embed().await {
        Ok(dimension) => dimension,
        Err(error) => {
            checkpoint.status = ProbeStatus::Failed;
            record_provider_error(&mut checkpoint, &error);
            checkpoint.request_count = budget.used();
            write_checkpoint(&checkpoint_path, &checkpoint).await?;
            return Err(ProbeError::Provider(error.code().to_owned()));
        }
    };
    checkpoint.dimension = Some(dimension);
    checkpoint.request_count = budget.used();
    let dimension_status = match config.embedding_expected_dimension {
        ExpectedEmbeddingDimension::Known {
            dimension: expected,
        } if dimension == expected => ProbeDimensionStatus::Matched,
        ExpectedEmbeddingDimension::Known { .. } => ProbeDimensionStatus::Mismatch,
        ExpectedEmbeddingDimension::Discover if dimension > 0 => ProbeDimensionStatus::Discovered,
        ExpectedEmbeddingDimension::Discover => ProbeDimensionStatus::Invalid,
    };
    checkpoint.dimension_status = Some(dimension_status);
    match dimension_status {
        ProbeDimensionStatus::Mismatch => {
            checkpoint.status = ProbeStatus::Failed;
            checkpoint.error_code = Some("embedding_dimension_mismatch".to_owned());
            write_checkpoint(&checkpoint_path, &checkpoint).await?;
            return Err(ProbeError::Provider(
                "embedding_dimension_mismatch".to_owned(),
            ));
        }
        ProbeDimensionStatus::Invalid => {
            checkpoint.status = ProbeStatus::Failed;
            checkpoint.error_code = Some("embedding_dimension_invalid".to_owned());
            write_checkpoint(&checkpoint_path, &checkpoint).await?;
            return Err(ProbeError::Provider(
                "embedding_dimension_invalid".to_owned(),
            ));
        }
        ProbeDimensionStatus::Matched | ProbeDimensionStatus::Discovered => {}
    }
    if checkpoint.request_count != 2 {
        return Err(ProbeError::BudgetExhausted);
    }
    checkpoint.stage = ProbeStage::Complete;
    checkpoint.status = ProbeStatus::Succeeded;
    write_checkpoint(&checkpoint_path, &checkpoint).await?;
    Ok(checkpoint)
}

fn base_checkpoint(config: &ProviderCapabilityProbeConfig) -> ProbeCheckpoint {
    ProbeCheckpoint {
        stage: ProbeStage::Preflight,
        status: ProbeStatus::Running,
        generation_provider_kind: config.generation.kind,
        embedding_provider_kind: config.embedding.kind,
        generation_model_id: config.generation.model_id.clone(),
        embedding_model_id: config.embedding.model_id.clone(),
        request_count: 0,
        error_code: None,
        http_status: None,
        usage_status: "not_observed".to_owned(),
        dimension: None,
        dimension_status: None,
    }
}

fn record_provider_error(checkpoint: &mut ProbeCheckpoint, error: &ProviderError) {
    checkpoint.error_code = Some(error.code().to_owned());
    checkpoint.http_status = match error {
        ProviderError::HttpStatus { status, .. } => Some(*status),
        _ => None,
    };
}

async fn write_checkpoint(path: &Path, checkpoint: &ProbeCheckpoint) -> Result<(), ProbeError> {
    let parent = path.parent().ok_or(ProbeError::Checkpoint)?;
    create_private_directory(parent).map_err(|_| ProbeError::Checkpoint)?;
    let bytes = serde_json::to_vec_pretty(checkpoint).map_err(|_| ProbeError::Checkpoint)?;
    private_fs::atomic_write_private_file(path, &bytes).map_err(|_| ProbeError::Checkpoint)
}

fn hmac_tag(key: &[u8], domain: &[u8], value: &[u8]) -> Result<[u8; 32], ProbeError> {
    let mut mac = HmacSha256::new_from_slice(key).map_err(|_| ProbeError::Credential)?;
    mac.update(domain);
    mac.update(value);
    let bytes = mac.finalize().into_bytes();
    let mut tag = [0_u8; 32];
    tag.copy_from_slice(&bytes);
    Ok(tag)
}

fn hex_tag(tag: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in tag {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

fn parse_hex_tag(value: &str) -> Result<[u8; 32], ProbeError> {
    if value.len() != 64 {
        return Err(ProbeError::Checkpoint);
    }
    let mut tag = [0_u8; 32];
    for (index, byte) in tag.iter_mut().enumerate() {
        let start = index * 2;
        *byte =
            u8::from_str_radix(&value[start..start + 2], 16).map_err(|_| ProbeError::Checkpoint)?;
    }
    Ok(tag)
}

fn verify_tag(key: &[u8], domain: &[u8], value: &[u8], tag: &[u8; 32]) -> bool {
    let Ok(mut mac) = HmacSha256::new_from_slice(key) else {
        return false;
    };
    mac.update(domain);
    mac.update(value);
    mac.verify_slice(tag).is_ok()
}

fn seal_path(config: &ProviderCapabilityProbeConfig) -> PathBuf {
    Path::new(&config.artifact_root).join(PREPARED_SEAL_FILE)
}

fn checkpoint_path(config: &ProviderCapabilityProbeConfig) -> PathBuf {
    Path::new(&config.artifact_root).join(CHECKPOINT_FILE)
}

fn write_prepared_seal(
    config: &ProviderCapabilityProbeConfig,
    key: &[u8],
    identity: &PreparedStateIdentity,
) -> Result<(), ProbeError> {
    let config_bytes = serde_json::to_vec(config).map_err(|_| ProbeError::Config)?;
    let config_tag = hmac_tag(key, b"mcp-vault/probe/config/v1\0", &config_bytes)?;
    let config_tag = hex_tag(&config_tag);
    let payload = PreparedStateSealPayload {
        schema_version: PREPARED_SEAL_SCHEMA,
        config_tag: &config_tag,
        vault_id: &identity.vault_id,
        generation_provider_id: &identity.generation_provider_id,
        embedding_provider_id: &identity.embedding_provider_id,
        generation_model_id: &identity.generation_model_id,
        embedding_model_id: &identity.embedding_model_id,
    };
    let payload_bytes = serde_json::to_vec(&payload).map_err(|_| ProbeError::Checkpoint)?;
    let seal_tag = hmac_tag(key, b"mcp-vault/probe/seal/v1\0", &payload_bytes)?;
    let seal = PreparedStateSeal {
        schema_version: PREPARED_SEAL_SCHEMA.to_owned(),
        config_tag,
        vault_id: identity.vault_id.clone(),
        generation_provider_id: identity.generation_provider_id.clone(),
        embedding_provider_id: identity.embedding_provider_id.clone(),
        generation_model_id: identity.generation_model_id.clone(),
        embedding_model_id: identity.embedding_model_id.clone(),
        seal_tag: hex_tag(&seal_tag),
    };
    let path = seal_path(config);
    let parent = path.parent().ok_or(ProbeError::Checkpoint)?;
    create_private_directory(parent).map_err(|_| ProbeError::Checkpoint)?;
    let bytes = serde_json::to_vec(&seal).map_err(|_| ProbeError::Checkpoint)?;
    private_fs::atomic_write_private_file(&path, &bytes).map_err(|_| ProbeError::Checkpoint)
}

fn load_prepared_key(
    config: &ProviderCapabilityProbeConfig,
) -> Result<zeroize::Zeroizing<Vec<u8>>, ProbeError> {
    let path = Path::new(&config.master_key_path);
    private_fs::validate_private_file(path).map_err(|_| ProbeError::UnsafeRoots)?;
    let bytes = zeroize::Zeroizing::new(fs::read(path).map_err(|_| ProbeError::Credential)?);
    if bytes.len() != 32 {
        return Err(ProbeError::Credential);
    }
    Ok(bytes)
}

fn validate_prepared_seal(
    config: &ProviderCapabilityProbeConfig,
    key: &[u8],
) -> Result<PreparedStateIdentity, ProbeError> {
    let seal = read_and_authenticate_prepared_seal(config, key)?;
    let stored_config_tag = parse_hex_tag(&seal.config_tag)?;
    let config_bytes = serde_json::to_vec(config).map_err(|_| ProbeError::Config)?;
    if !verify_tag(
        key,
        b"mcp-vault/probe/config/v1\0",
        &config_bytes,
        &stored_config_tag,
    ) {
        return Err(ProbeError::Config);
    }
    Ok(PreparedStateIdentity {
        vault_id: seal.vault_id,
        generation_provider_id: seal.generation_provider_id,
        embedding_provider_id: seal.embedding_provider_id,
        generation_model_id: seal.generation_model_id,
        embedding_model_id: seal.embedding_model_id,
    })
}

fn read_and_authenticate_prepared_seal(
    config: &ProviderCapabilityProbeConfig,
    key: &[u8],
) -> Result<PreparedStateSeal, ProbeError> {
    let path = seal_path(config);
    private_fs::validate_private_file(&path).map_err(|_| ProbeError::UnsafeRoots)?;
    let seal: PreparedStateSeal =
        serde_json::from_slice(&fs::read(&path).map_err(|_| ProbeError::Checkpoint)?)
            .map_err(|_| ProbeError::Checkpoint)?;
    if seal.schema_version != PREPARED_SEAL_SCHEMA {
        return Err(ProbeError::Checkpoint);
    }
    let _stored_config_tag = parse_hex_tag(&seal.config_tag)?;
    let stored_seal_tag = parse_hex_tag(&seal.seal_tag)?;
    let payload = PreparedStateSealPayload {
        schema_version: &seal.schema_version,
        config_tag: &seal.config_tag,
        vault_id: &seal.vault_id,
        generation_provider_id: &seal.generation_provider_id,
        embedding_provider_id: &seal.embedding_provider_id,
        generation_model_id: &seal.generation_model_id,
        embedding_model_id: &seal.embedding_model_id,
    };
    let payload_bytes = serde_json::to_vec(&payload).map_err(|_| ProbeError::Checkpoint)?;
    if !verify_tag(
        key,
        b"mcp-vault/probe/seal/v1\0",
        &payload_bytes,
        &stored_seal_tag,
    ) {
        return Err(ProbeError::Checkpoint);
    }
    Ok(seal)
}

fn expected_prepared_directories(config: &ProviderCapabilityProbeConfig) -> BTreeSet<PathBuf> {
    let run = Path::new(&config.run_root);
    let directory_leaves = [
        Path::new(&config.state_root),
        Path::new(&config.vault_root),
        Path::new(&config.history_root),
        Path::new(&config.artifact_root),
        Path::new(&config.master_key_path).parent().unwrap_or(run),
    ];
    let mut expected = BTreeSet::from([run.to_owned()]);
    for leaf in directory_leaves {
        let mut current = leaf;
        while current.starts_with(run) && current != run {
            expected.insert(current.to_owned());
            let Some(parent) = current.parent() else {
                break;
            };
            current = parent;
        }
    }
    expected
}

fn expected_prepared_files(config: &ProviderCapabilityProbeConfig) -> BTreeSet<PathBuf> {
    let state_db = Path::new(&config.state_root).join("state.sqlite3");
    BTreeSet::from([
        Path::new(&config.master_key_path).to_owned(),
        checkpoint_path(config),
        seal_path(config),
        state_db.clone(),
        PathBuf::from(format!("{}-wal", state_db.display())),
        PathBuf::from(format!("{}-shm", state_db.display())),
        PathBuf::from(format!("{}-journal", state_db.display())),
    ])
}

fn validate_prepared_tree(
    path: &Path,
    expected_directories: &BTreeSet<PathBuf>,
    expected_files: &BTreeSet<PathBuf>,
) -> Result<(), ProbeError> {
    let entries = fs::read_dir(path).map_err(|_| ProbeError::UnsafeRoots)?;
    for entry in entries {
        let entry = entry.map_err(|_| ProbeError::UnsafeRoots)?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child).map_err(|_| ProbeError::UnsafeRoots)?;
        if metadata.file_type().is_symlink() {
            return Err(ProbeError::UnsafeRoots);
        }
        if metadata.is_dir() {
            if !expected_directories.contains(&child) {
                return Err(ProbeError::UnsafeRoots);
            }
            private_fs::validate_private_directory(&child).map_err(|_| ProbeError::UnsafeRoots)?;
            validate_prepared_tree(&child, expected_directories, expected_files)?;
        } else if metadata.is_file() {
            if !expected_files.contains(&child) {
                return Err(ProbeError::UnsafeRoots);
            }
            private_fs::validate_private_file(&child).map_err(|_| ProbeError::UnsafeRoots)?;
        } else {
            return Err(ProbeError::UnsafeRoots);
        }
    }
    Ok(())
}

fn validate_prepared_filesystem(config: &ProviderCapabilityProbeConfig) -> Result<(), ProbeError> {
    let run = Path::new(&config.run_root);
    let roots = [
        Path::new(&config.state_root),
        Path::new(&config.vault_root),
        Path::new(&config.history_root),
        Path::new(&config.artifact_root),
        Path::new(&config.master_key_path)
            .parent()
            .ok_or(ProbeError::UnsafeRoots)?,
    ];
    private_fs::validate_private_directory(run).map_err(|_| ProbeError::UnsafeRoots)?;
    for root in roots {
        private_fs::validate_private_directory(root).map_err(|_| ProbeError::UnsafeRoots)?;
    }
    let state_db = Path::new(&config.state_root).join("state.sqlite3");
    private_fs::validate_private_file(&state_db).map_err(|_| ProbeError::UnsafeRoots)?;
    private_fs::validate_private_file(Path::new(&config.master_key_path))
        .map_err(|_| ProbeError::UnsafeRoots)?;
    private_fs::validate_private_file(&checkpoint_path(config))
        .map_err(|_| ProbeError::UnsafeRoots)?;
    private_fs::validate_private_file(&seal_path(config)).map_err(|_| ProbeError::UnsafeRoots)?;
    if fs::metadata(&state_db)
        .map_err(|_| ProbeError::UnsafeRoots)?
        .len()
        == 0
    {
        return Err(ProbeError::State);
    }
    let expected_directories = expected_prepared_directories(config);
    let expected_files = expected_prepared_files(config);
    validate_prepared_tree(run, &expected_directories, &expected_files)?;
    ensure_directory_empty(Path::new(&config.vault_root))?;
    ensure_directory_empty(Path::new(&config.history_root))?;
    Ok(())
}

fn private_state_files(state_root: &Path) -> Result<(), ProbeError> {
    let entries = fs::read_dir(state_root).map_err(|_| ProbeError::UnsafeRoots)?;
    for entry in entries {
        let path = entry.map_err(|_| ProbeError::UnsafeRoots)?.path();
        private_fs::validate_private_file(&path).map_err(|_| ProbeError::UnsafeRoots)?;
    }
    Ok(())
}

fn set_private_state_file_modes(state_root: &Path) -> Result<(), ProbeError> {
    #[cfg(unix)]
    {
        for entry in fs::read_dir(state_root).map_err(|_| ProbeError::UnsafeRoots)? {
            let path = entry.map_err(|_| ProbeError::UnsafeRoots)?.path();
            let metadata = fs::symlink_metadata(&path).map_err(|_| ProbeError::UnsafeRoots)?;
            if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.nlink() != 1 {
                return Err(ProbeError::UnsafeRoots);
            }
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .map_err(|_| ProbeError::UnsafeRoots)?;
            private_fs::validate_private_file(&path).map_err(|_| ProbeError::UnsafeRoots)?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = state_root;
        Err(ProbeError::UnsafeRoots)
    }
}

async fn read_prepared_checkpoint(
    config: &ProviderCapabilityProbeConfig,
) -> Result<ProbeCheckpoint, ProbeError> {
    let bytes = async_fs::read(checkpoint_path(config))
        .await
        .map_err(|_| ProbeError::Checkpoint)?;
    let existing = parse_checkpoint(&bytes)?;
    if existing.stage != ProbeStage::Provisioning
        || existing.status != ProbeStatus::Running
        || existing.generation_provider_kind != config.generation.kind
        || existing.embedding_provider_kind != config.embedding.kind
        || existing.generation_model_id != config.generation.model_id
        || existing.embedding_model_id != config.embedding.model_id
        || existing.request_count != 0
        || existing.error_code.is_some()
        || existing.http_status.is_some()
        || existing.usage_status != "not_observed"
        || existing.dimension.is_some()
        || existing.dimension_status.is_some()
    {
        return Err(ProbeError::Checkpoint);
    }
    Ok(existing)
}

fn parse_checkpoint(bytes: &[u8]) -> Result<ProbeCheckpoint, ProbeError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| ProbeError::Checkpoint)?;
    if !value
        .as_object()
        .is_some_and(|object| object.contains_key("http_status"))
    {
        // Prepared checkpoints written before this field was introduced must
        // not be accepted as a current zero-request run.
        return Err(ProbeError::Checkpoint);
    }
    serde_json::from_value(value).map_err(|_| ProbeError::Checkpoint)
}

async fn validate_prepared_probe_run(
    config: &ProviderCapabilityProbeConfig,
) -> Result<ProbeCheckpoint, ProbeError> {
    validate_probe_config_fields(config)?;
    validate_prepared_filesystem(config)?;
    let key = load_prepared_key(config)?;
    let identity = validate_prepared_seal(config, &key)?;
    let checkpoint = read_prepared_checkpoint(config).await?;
    validate_prepared_database(config, &identity).await?;
    private_state_files(Path::new(&config.state_root))?;
    Ok(checkpoint)
}

async fn validate_prepared_database(
    config: &ProviderCapabilityProbeConfig,
    identity: &PreparedStateIdentity,
) -> Result<(), ProbeError> {
    let state_db = Path::new(&config.state_root).join("state.sqlite3");
    let state = StateStore::connect_read_only(&format!("sqlite://{}", state_db.display()))
        .await
        .map_err(|_| ProbeError::State)?;
    let integrity = state
        .integrity_check()
        .await
        .map_err(|_| ProbeError::State)?;
    if !integrity.integrity_ok
        || integrity.foreign_key_violations != 0
        || integrity.migration_version != CURRENT_STATE_MIGRATION
    {
        return Err(ProbeError::State);
    }

    let slug = VaultSlug::new(&config.vault_slug).map_err(|_| ProbeError::Config)?;
    let vaults = state.vaults().list().await.map_err(|_| ProbeError::State)?;
    if vaults.len() != 1 {
        return Err(ProbeError::State);
    }
    let vault = &vaults[0];
    if vault.slug != slug
        || vault.id.to_string().as_str() != identity.vault_id.as_str()
        || vault.name != "Provider capability probe"
        || vault.status != mcp_vault_state::VaultStatus::Active
        || vault.content_root != Path::new(&config.vault_root)
        || &vault.reserved_root != mcp_vault_domain::VaultPathPolicy::default().reserved_root()
    {
        return Err(ProbeError::State);
    }
    let context = vault.context().map_err(|_| ProbeError::State)?;
    let provider_mode = state
        .settings()
        .get_vault(&context, "provider.mode")
        .await
        .map_err(|_| ProbeError::State)?
        .ok_or(ProbeError::State)?;
    if provider_mode.value
        != serde_json::to_value(config.provider_mode).map_err(|_| ProbeError::Config)?
    {
        return Err(ProbeError::Config);
    }

    let providers = state
        .providers()
        .list_providers(10)
        .await
        .map_err(|_| ProbeError::State)?;
    let models = state
        .providers()
        .list_models(None, 10)
        .await
        .map_err(|_| ProbeError::State)?;
    if providers.len() != 2 || models.len() != 2 {
        return Err(ProbeError::State);
    }
    for (
        provider_config,
        expected_has_secret,
        expected_provider_id,
        expected_model_id,
        expected_model_settings,
    ) in [
        (
            &config.generation,
            config
                .credential_provisioning
                .as_ref()
                .and_then(|sources| sources.generation.as_ref())
                .is_some(),
            &identity.generation_provider_id,
            &identity.generation_model_id,
            generation_probe_model_settings(),
        ),
        (
            &config.embedding,
            config
                .credential_provisioning
                .as_ref()
                .and_then(|sources| sources.embedding.as_ref())
                .is_some(),
            &identity.embedding_provider_id,
            &identity.embedding_model_id,
            ModelSettings::default(),
        ),
    ] {
        let base_url = Url::parse(&provider_config.base_url)
            .map_err(|_| ProbeError::Config)?
            .to_string();
        let provider = providers
            .iter()
            .find(|provider| {
                provider.name == provider_config.name
                    && provider.provider_type == provider_config.kind.as_str()
                    && provider.base_url == base_url
            })
            .ok_or(ProbeError::State)?;
        let expected_settings = ProviderSettings {
            max_retries: 0,
            ..ProviderSettings::default()
        };
        if !provider.enabled
            || provider.id.to_string().as_str() != expected_provider_id.as_str()
            || provider.secret_id.is_some() != expected_has_secret
            || serde_json::from_value::<ProviderSettings>(provider.settings.clone())
                .map_err(|_| ProbeError::State)?
                != expected_settings
        {
            return Err(ProbeError::State);
        }
        let expected_capabilities =
            serde_json::to_value(&provider_config.capabilities).map_err(|_| ProbeError::Config)?;
        let matching_models: Vec<_> = models
            .iter()
            .filter(|model| model.provider_id == provider.id)
            .collect();
        if matching_models.len() != 1 {
            return Err(ProbeError::State);
        }
        let model = matching_models[0];
        if !model.enabled
            || model.id.to_string().as_str() != expected_model_id.as_str()
            || model.external_model_id != provider_config.model_id
            || model.capabilities != expected_capabilities
            || model.settings
                != serde_json::to_value(expected_model_settings).map_err(|_| ProbeError::Config)?
        {
            return Err(ProbeError::State);
        }
    }
    Ok(())
}

async fn record_prepared_validation_failure(
    config: &ProviderCapabilityProbeConfig,
    error: &ProbeError,
) -> Result<(), ProbeError> {
    let artifact_root = Path::new(&config.artifact_root);
    private_fs::validate_private_directory(artifact_root).map_err(|_| ProbeError::Checkpoint)?;
    let path = checkpoint_path(config);
    private_fs::validate_private_file(&path).map_err(|_| ProbeError::Checkpoint)?;
    let bytes = async_fs::read(&path)
        .await
        .map_err(|_| ProbeError::Checkpoint)?;
    let mut checkpoint = parse_checkpoint(&bytes)?;
    if checkpoint.stage != ProbeStage::Provisioning
        || checkpoint.status != ProbeStatus::Running
        || checkpoint.request_count != 0
        || checkpoint.error_code.is_some()
        || checkpoint.http_status.is_some()
        || checkpoint.usage_status != "not_observed"
        || checkpoint.dimension.is_some()
        || checkpoint.dimension_status.is_some()
    {
        return Err(ProbeError::Checkpoint);
    }
    checkpoint.stage = ProbeStage::Provisioning;
    checkpoint.status = ProbeStatus::Failed;
    checkpoint.error_code = Some(error.safe_code().to_owned());
    write_checkpoint(&path, &checkpoint).await
}

/// Prepare the isolated destination and return the Provider boundary.  This
/// is called only by the explicit CLI flag after preflight validation.
pub async fn prepare_provider_capability_probe(
    config: &ProviderCapabilityProbeConfig,
) -> Result<(ProviderServiceCapabilityBoundary, Arc<ProbeRequestBudget>), ProbeError> {
    validate_provider_capability_probe_config(config)?;
    let run_root = PathBuf::from(&config.run_root);
    create_private_directory(&run_root)?;
    validate_probe_config_fields(config)?;
    ensure_directory_empty(&run_root)?;
    for root in [
        &config.state_root,
        &config.vault_root,
        &config.history_root,
        &config.artifact_root,
    ] {
        create_private_directory(Path::new(root))?;
    }
    validate_probe_config_fields(config)?;
    for root in [
        &config.state_root,
        &config.vault_root,
        &config.history_root,
        &config.artifact_root,
    ] {
        ensure_directory_empty(Path::new(root))?;
    }
    let state_db = Path::new(&config.state_root).join("state.sqlite3");
    let key_path = Path::new(&config.master_key_path);
    if lstat_if_exists(&state_db)?.is_some() || lstat_if_exists(key_path)?.is_some() {
        return Err(ProbeError::UnsafeRoots);
    }
    let key_parent = key_path.parent().ok_or(ProbeError::UnsafeRoots)?;
    create_private_directory(key_parent)?;
    validate_probe_config_fields(config)?;
    validate_private_directory(key_parent, 0o700)?;
    ensure_directory_empty(key_parent)?;
    let checkpoint_path = checkpoint_path(config);
    if async_fs::try_exists(&checkpoint_path)
        .await
        .map_err(|_| ProbeError::Checkpoint)?
    {
        return Err(ProbeError::Checkpoint);
    }
    let mut checkpoint = base_checkpoint(config);
    write_checkpoint(&checkpoint_path, &checkpoint).await?;
    checkpoint.stage = ProbeStage::Provisioning;
    write_checkpoint(&checkpoint_path, &checkpoint).await?;
    let result = async {
        let state = StateStore::connect_and_migrate(&format!("sqlite://{}", state_db.display()))
            .await
            .map_err(|_| ProbeError::State)?;
        let integrity = state
            .integrity_check()
            .await
            .map_err(|_| ProbeError::State)?;
        if !integrity.integrity_ok
            || integrity.foreign_key_violations != 0
            || integrity.migration_version != CURRENT_STATE_MIGRATION
        {
            return Err(ProbeError::State);
        }
        let (key, key_bytes) = create_probe_master_key(Path::new(&config.master_key_path))?;
        let auth = AuthService::new(state.auth(), key);
        let managed = ManagedVaultService::new(
            state.clone(),
            run_root.clone(),
            StorageOptions {
                durability: DurabilityPolicy::None,
                minimum_free_bytes: 0,
                directory_mode: 0o700,
                ..StorageOptions::default()
            },
        );
        let slug = VaultSlug::new(&config.vault_slug).map_err(|_| ProbeError::Config)?;
        let vault = managed
            .create(slug, "Provider capability probe")
            .await
            .map_err(|_| ProbeError::State)?
            .vault;
        let context = vault.context().map_err(|_| ProbeError::State)?;
        if context.content_root() != Path::new(&config.vault_root) {
            return Err(ProbeError::UnsafeRoots);
        }
        let budget = Arc::new(ProbeRequestBudget::new());
        let service = ProviderService::new(state.clone(), auth.clone())
            .with_generation_budget(budget.clone());
        service
            .set_provider_mode(&context, config.provider_mode, None)
            .await
            .map_err(|_| ProbeError::State)?;
        let generation_secret = read_source_secret(config, true).await?;
        let embedding_secret = read_source_secret(config, false).await?;
        let generation_provider =
            create_probe_provider(&service, &config.generation, generation_secret).await?;
        let embedding_provider =
            create_probe_provider(&service, &config.embedding, embedding_secret).await?;
        let generation_model = service
            .register_model(ModelInput {
                provider_id: generation_provider.id,
                external_model_id: config.generation.model_id.clone(),
                capabilities: config.generation.capabilities.clone(),
                settings: generation_probe_model_settings(),
                enabled: true,
            })
            .await
            .map_err(|_| ProbeError::State)?;
        let embedding_model = service
            .register_model(ModelInput {
                provider_id: embedding_provider.id,
                external_model_id: config.embedding.model_id.clone(),
                capabilities: config.embedding.capabilities.clone(),
                settings: ModelSettings::default(),
                enabled: true,
            })
            .await
            .map_err(|_| ProbeError::State)?;
        set_private_state_file_modes(Path::new(&config.state_root))?;
        let identity = PreparedStateIdentity {
            vault_id: vault.id.to_string(),
            generation_provider_id: generation_provider.id.to_string(),
            embedding_provider_id: embedding_provider.id.to_string(),
            generation_model_id: generation_model.id.to_string(),
            embedding_model_id: embedding_model.id.to_string(),
        };
        write_prepared_seal(config, &key_bytes[..], &identity)?;
        Ok((
            ProviderServiceCapabilityBoundary::new(
                service,
                context,
                generation_model.id,
                embedding_model.id,
                generation_model.external_model_id,
                embedding_model.external_model_id,
                budget.clone(),
            ),
            budget,
        ))
    }
    .await;
    if let Err(error) = &result {
        checkpoint.status = ProbeStatus::Failed;
        checkpoint.error_code = Some(error.safe_code().to_owned());
        write_checkpoint(&checkpoint_path, &checkpoint).await?;
    }
    result
}

async fn create_probe_provider(
    service: &ProviderService,
    config: &ProbeProviderConfig,
    secret: Option<SecretString>,
) -> Result<mcp_vault_state::ProviderRecord, ProbeError> {
    let url = Url::parse(&config.base_url).map_err(|_| ProbeError::Config)?;
    let settings = ProviderSettings {
        max_retries: 0,
        ..ProviderSettings::default()
    };
    service
        .create_provider(ProviderInput {
            name: config.name.clone(),
            kind: config.kind,
            base_url: url,
            settings,
            enabled: true,
            secret,
        })
        .await
        .map_err(|_| ProbeError::State)
}

async fn read_source_secret(
    config: &ProviderCapabilityProbeConfig,
    generation: bool,
) -> Result<Option<SecretString>, ProbeError> {
    let Some(source) = config.credential_provisioning.as_ref().and_then(|sources| {
        if generation {
            sources.generation.as_ref()
        } else {
            sources.embedding.as_ref()
        }
    }) else {
        return Ok(None);
    };
    let source_state =
        StateStore::connect_read_only(&format!("sqlite://{}", source.source_database_path))
            .await
            .map_err(|_| ProbeError::SourceStateOpenFailed)?;
    let source_key = MasterKeyRing::load_file(Path::new(&source.source_master_key_path))
        .await
        .map_err(|_| ProbeError::SourceMasterKeyLoadFailed)?;
    let source_auth = AuthService::new(source_state.auth(), source_key);
    let provider_id = ProviderId::parse(&source.source_provider_id)
        .map_err(|_| ProbeError::SourceSecretReferenceInvalid)?;
    let provider_reference = source_state
        .providers()
        .get_provider_secret_reference(provider_id)
        .await
        .map_err(|error| match error {
            mcp_vault_state::StateError::SchemaIncompatible => {
                ProbeError::SourceProviderSchemaIncompatible
            }
            mcp_vault_state::StateError::InvalidDomain(_) => {
                ProbeError::SourceSecretMetadataInvalid
            }
            _ => ProbeError::SourceProviderReferenceUnavailable,
        })?
        .ok_or(ProbeError::SourceProviderReferenceUnavailable)?;
    let secret_id = provider_reference.secret_id;
    let secret_record = source_state
        .auth()
        .get_secret(secret_id)
        .await
        .map_err(|_| ProbeError::SourceSecretMetadataInvalid)?
        .ok_or(ProbeError::SourceSecretReferenceInvalid)?;
    let expected_owner_id = provider_reference.provider_id.to_string();
    if secret_record.purpose != PROVIDER_SECRET_PURPOSE
        || secret_record.owner_type != PROVIDER_SECRET_OWNER
        || secret_record.owner_id.as_deref() != Some(expected_owner_id.as_str())
    {
        return Err(ProbeError::SourceSecretMetadataInvalid);
    }
    source_auth
        .read_installation_secret(
            secret_id,
            PROVIDER_SECRET_PURPOSE,
            PROVIDER_SECRET_OWNER,
            Some(&expected_owner_id),
        )
        .await
        .map(Some)
        .map_err(|error| match error {
            mcp_vault_auth::AuthError::State(_) => ProbeError::SourceSecretMetadataInvalid,
            _ => ProbeError::SourceSecretDecryptFailed,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, routing::post};
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::sync::Mutex;
    use tempfile::tempdir;
    use tokio::net::TcpListener;

    async fn capture_probe_generation(
        State(captured): State<Arc<Mutex<Vec<serde_json::Value>>>>,
        Json(request): Json<serde_json::Value>,
    ) -> axum::response::Response {
        captured.lock().unwrap().push(request);
        axum::response::Response::builder()
            .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
            .body(axum::body::Body::from(concat!(
                "data: {\"id\":\"synthetic-probe-response\",\"model\":\"mimo-v2.5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"{\\\"ok\\\":true}\"}}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            )))
            .unwrap()
    }

    async fn return_probe_embedding() -> Json<serde_json::Value> {
        Json(json!({
            "model": "synthetic-embedding",
            "data": [{"index": 0, "embedding": [0.1, 0.2, 0.3]}]
        }))
    }

    fn config(root: &Path) -> ProviderCapabilityProbeConfig {
        let root = fs::canonicalize(root).unwrap().join("probe-run");
        let provider = |name: &str, model: &str, embedding: bool| ProbeProviderConfig {
            name: name.to_owned(),
            kind: if embedding {
                ProviderKind::EmbeddingHttp
            } else {
                ProviderKind::XiaomiMimo
            },
            base_url: "http://127.0.0.1:9/v1/".to_owned(),
            model_id: model.to_owned(),
            capabilities: ModelCapabilities {
                structured_output: !embedding,
                embeddings: embedding,
                dimension: embedding.then_some(3),
                ..Default::default()
            },
        };
        ProviderCapabilityProbeConfig {
            schema_version: PROVIDER_CAPABILITY_PROBE_SCHEMA.to_owned(),
            run_root: root.display().to_string(),
            state_root: root.join("state").display().to_string(),
            vault_root: root.join("vaults/probe").display().to_string(),
            history_root: root.join("history").display().to_string(),
            artifact_root: root.join("artifacts").display().to_string(),
            master_key_path: root.join("keys/master").display().to_string(),
            vault_slug: "probe".to_owned(),
            provider_mode: ProviderMode::LocalOnly,
            generation: provider("generation", "synthetic-generation", false),
            embedding: provider("embedding", "synthetic-embedding", true),
            embedding_expected_dimension: ExpectedEmbeddingDimension::Known { dimension: 3 },
            credential_provisioning: None,
        }
    }

    #[cfg(unix)]
    async fn prepare_fake_probe(config: &ProviderCapabilityProbeConfig) {
        let _ = prepare_provider_capability_probe(config).await.unwrap();
    }

    #[tokio::test]
    async fn source_credential_errors_write_only_stable_checkpoint_codes() {
        let dir = tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let cfg = config(&root);
        let artifact_root = root.join("artifacts");
        create_private_directory(&artifact_root).unwrap();
        let cases = [
            (
                ProbeError::SourceStateOpenFailed,
                "source_state_open_failed",
            ),
            (
                ProbeError::SourceMasterKeyLoadFailed,
                "source_master_key_load_failed",
            ),
            (
                ProbeError::SourceProviderReferenceUnavailable,
                "source_provider_reference_unavailable",
            ),
            (
                ProbeError::SourceProviderSchemaIncompatible,
                "source_provider_schema_incompatible",
            ),
            (
                ProbeError::SourceSecretReferenceInvalid,
                "source_secret_reference_invalid",
            ),
            (
                ProbeError::SourceSecretMetadataInvalid,
                "source_secret_metadata_invalid",
            ),
            (
                ProbeError::SourceSecretDecryptFailed,
                "source_secret_decrypt_failed",
            ),
        ];

        for (index, (error, expected_code)) in cases.into_iter().enumerate() {
            let mut checkpoint = base_checkpoint(&cfg);
            checkpoint.status = ProbeStatus::Failed;
            checkpoint.error_code = Some(error.safe_code().to_owned());
            let path = artifact_root.join(format!("checkpoint-{index}.json"));
            write_checkpoint(&path, &checkpoint).await.unwrap();

            let bytes = fs::read(&path).unwrap();
            let serialized = String::from_utf8(bytes.clone()).unwrap();
            let checkpoint_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(checkpoint_json["error_code"], expected_code);
            assert!(!serialized.contains(&error.to_string()));
            assert!(!serialized.contains("ciphertext"));
            assert!(!serialized.contains("source-master-key"));
        }

        assert_eq!(ProbeError::Credential.safe_code(), "probe_credential_error");
        assert_eq!(ProbeError::State.safe_code(), "probe_state_error");
    }

    #[tokio::test]
    async fn source_secret_read_uses_legacy_provider_reference_without_full_provider_query() {
        use mcp_vault_domain::SecretId;

        let dir = tempdir().unwrap();
        let source_root = dir.path().join("legacy-source");
        fs::create_dir(&source_root).unwrap();
        let source_db = source_root.join("state.sqlite3");
        let source_key_path = source_root.join("master-key");
        let provider_id = ProviderId::new();
        let secret_id = SecretId::new();
        let source_key = MasterKeyRing::from_bytes(1, &[37_u8; 32]).unwrap();
        let encrypted = source_key
            .encrypt(
                PROVIDER_SECRET_PURPOSE,
                PROVIDER_SECRET_OWNER,
                Some(&provider_id.to_string()),
                b"synthetic-fixture-secret",
            )
            .unwrap();
        fs::write(&source_key_path, [37_u8; 32]).unwrap();

        let fixture_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&source_db)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE providers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                provider_type TEXT NOT NULL,
                base_url TEXT NOT NULL,
                secret_id TEXT,
                settings_json TEXT NOT NULL,
                enabled INTEGER NOT NULL,
                revision INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&fixture_pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE encrypted_secrets (
                id TEXT PRIMARY KEY,
                purpose TEXT NOT NULL,
                owner_type TEXT NOT NULL,
                owner_id TEXT,
                key_version INTEGER NOT NULL,
                nonce BLOB NOT NULL,
                ciphertext BLOB NOT NULL,
                hint TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&fixture_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO encrypted_secrets
             (id, purpose, owner_type, owner_id, key_version, nonce, ciphertext,
              hint, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, NULL, 1, 1)",
        )
        .bind(secret_id.to_string())
        .bind(PROVIDER_SECRET_PURPOSE)
        .bind(PROVIDER_SECRET_OWNER)
        .bind(provider_id.to_string())
        .bind(i64::from(encrypted.key_version))
        .bind(encrypted.nonce.as_slice())
        .bind(&encrypted.ciphertext)
        .execute(&fixture_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO providers
             (id, name, provider_type, base_url, secret_id, settings_json,
              enabled, revision, created_at, updated_at)
             VALUES (?, 'legacy fixture', 'openai_compatible', 'http://localhost', ?, '{}', 1, 1, 1, 1)",
        )
        .bind(provider_id.to_string())
        .bind(secret_id.to_string())
        .execute(&fixture_pool)
        .await
        .unwrap();
        fixture_pool.close().await;

        let before = fs::metadata(&source_db).unwrap();
        let read_only_state =
            StateStore::connect_read_only(&format!("sqlite://{}", source_db.display()))
                .await
                .unwrap();
        assert!(
            read_only_state
                .providers()
                .get_provider(provider_id)
                .await
                .is_err(),
            "the full current Provider query requires embedding_revision"
        );
        read_only_state.close().await;

        let mut cfg = config(dir.path());
        cfg.credential_provisioning = Some(ProbeCredentialProvisioning {
            generation: Some(ProbeSourceCredential {
                source_database_path: source_db.display().to_string(),
                source_master_key_path: source_key_path.display().to_string(),
                source_provider_id: provider_id.to_string(),
            }),
            embedding: None,
        });
        let secret = read_source_secret(&cfg, true).await.unwrap().unwrap();
        assert_eq!(secret.expose_secret(), "synthetic-fixture-secret");
        assert_eq!(format!("{secret:?}"), "[REDACTED]");
        let after = fs::metadata(&source_db).unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    }

    #[tokio::test]
    async fn source_provider_reference_absence_and_schema_mismatch_have_distinct_codes() {
        let dir = tempdir().unwrap();
        let source_key_path = dir.path().join("source-master-key");
        fs::write(&source_key_path, [41_u8; 32]).unwrap();

        for (name, provider_ddl, expected_code) in [
            (
                "missing-reference",
                "CREATE TABLE providers (id TEXT PRIMARY KEY, secret_id TEXT)",
                "source_provider_reference_unavailable",
            ),
            (
                "incompatible-schema",
                "CREATE TABLE providers (id TEXT PRIMARY KEY)",
                "source_provider_schema_incompatible",
            ),
        ] {
            let source_dir = dir.path().join(name);
            fs::create_dir(&source_dir).unwrap();
            let source_db = source_dir.join("state.sqlite3");
            let fixture_pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(
                    SqliteConnectOptions::new()
                        .filename(&source_db)
                        .create_if_missing(true),
                )
                .await
                .unwrap();
            sqlx::query(provider_ddl)
                .execute(&fixture_pool)
                .await
                .unwrap();
            fixture_pool.close().await;

            let mut cfg = config(dir.path());
            cfg.credential_provisioning = Some(ProbeCredentialProvisioning {
                generation: Some(ProbeSourceCredential {
                    source_database_path: source_db.display().to_string(),
                    source_master_key_path: source_key_path.display().to_string(),
                    source_provider_id: ProviderId::new().to_string(),
                }),
                embedding: None,
            });
            let error = read_source_secret(&cfg, true).await.unwrap_err();
            assert_eq!(error.safe_code(), expected_code);
            assert!(!error.to_string().contains("providers"));
        }
    }

    struct Fake {
        generation_calls: AtomicU32,
        embedding_calls: AtomicU32,
        fail_generation: bool,
        fail_embedding: bool,
        embedding_dimension: u32,
        generation_value: serde_json::Value,
        budget: Arc<ProbeRequestBudget>,
    }
    #[async_trait]
    impl CapabilityProbeProvider for Fake {
        async fn generate(&self) -> Result<StructuredGenerationResult, ProviderError> {
            self.generation_calls.fetch_add(1, Ordering::SeqCst);
            self.budget.reserve(1).await?;
            if self.fail_generation {
                return Err(ProviderError::InvalidResponse("response is not JSON"));
            }
            Ok(StructuredGenerationResult {
                value: self.generation_value.clone(),
                model: None,
                usage: None,
            })
        }
        async fn embed(&self) -> Result<u32, ProviderError> {
            self.embedding_calls.fetch_add(1, Ordering::SeqCst);
            self.budget.reserve(1).await?;
            if self.fail_embedding {
                return Err(ProviderError::InvalidResponse(
                    "embedding response is invalid",
                ));
            }
            Ok(self.embedding_dimension)
        }
    }

    struct HttpStatusFake {
        generation_calls: AtomicU32,
        embedding_calls: AtomicU32,
        generation_status: Option<u16>,
        embedding_status: Option<u16>,
        budget: Arc<ProbeRequestBudget>,
    }

    #[async_trait]
    impl CapabilityProbeProvider for HttpStatusFake {
        async fn generate(&self) -> Result<StructuredGenerationResult, ProviderError> {
            self.generation_calls.fetch_add(1, Ordering::SeqCst);
            self.budget.reserve(1).await?;
            if let Some(status) = self.generation_status {
                return Err(ProviderError::HttpStatus {
                    status,
                    retryable: false,
                });
            }
            Ok(StructuredGenerationResult {
                value: json!({"ok": true}),
                model: None,
                usage: None,
            })
        }

        async fn embed(&self) -> Result<u32, ProviderError> {
            self.embedding_calls.fetch_add(1, Ordering::SeqCst);
            self.budget.reserve(1).await?;
            if let Some(status) = self.embedding_status {
                return Err(ProviderError::HttpStatus {
                    status,
                    retryable: false,
                });
            }
            Ok(3)
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn generation_failure_stops_before_embedding_and_checkpoint_is_redacted() {
        let dir = tempdir().unwrap();
        let cfg = config(dir.path());
        let budget = Arc::new(ProbeRequestBudget::new());
        let fake = Fake {
            generation_calls: AtomicU32::new(0),
            embedding_calls: AtomicU32::new(0),
            fail_generation: true,
            fail_embedding: false,
            embedding_dimension: 3,
            generation_value: json!({"ok": true}),
            budget: budget.clone(),
        };
        prepare_fake_probe(&cfg).await;
        assert!(run_capability_probe(&cfg, &fake, budget).await.is_err());
        assert_eq!(fake.generation_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fake.embedding_calls.load(Ordering::SeqCst), 0);
        let text = fs::read_to_string(
            Path::new(&cfg.artifact_root).join("provider-capability-probe.checkpoint.json"),
        )
        .unwrap();
        let checkpoint: ProbeCheckpoint =
            serde_json::from_slice(&fs::read(checkpoint_path(&cfg)).unwrap()).unwrap();
        assert_eq!(checkpoint.http_status, None);
        for forbidden in [
            "synthetic input",
            "Authorization",
            "api_key",
            "response is not JSON",
        ] {
            assert!(!text.contains(forbidden), "checkpoint leaked {forbidden}");
        }
        assert!(text.contains("provider_response_json_invalid"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn provider_http_status_is_recorded_for_generation_and_embedding_failures() {
        for (generation_status, embedding_status, expected_stage, expected_requests) in [
            (Some(418), None, ProbeStage::Generation, 1),
            (None, Some(422), ProbeStage::Embedding, 2),
        ] {
            let dir = tempdir().unwrap();
            let cfg = config(dir.path());
            let budget = Arc::new(ProbeRequestBudget::new());
            let fake = HttpStatusFake {
                generation_calls: AtomicU32::new(0),
                embedding_calls: AtomicU32::new(0),
                generation_status,
                embedding_status,
                budget: budget.clone(),
            };

            prepare_fake_probe(&cfg).await;
            assert!(
                run_capability_probe(&cfg, &fake, budget.clone())
                    .await
                    .is_err()
            );

            let bytes = fs::read(checkpoint_path(&cfg)).unwrap();
            let checkpoint: ProbeCheckpoint = serde_json::from_slice(&bytes).unwrap();
            let expected_status = generation_status.or(embedding_status).unwrap();
            let checkpoint_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(checkpoint.stage, expected_stage);
            assert_eq!(checkpoint.status, ProbeStatus::Failed);
            assert_eq!(checkpoint.request_count, expected_requests);
            assert_eq!(
                checkpoint.error_code.as_deref(),
                Some("provider_http_error")
            );
            assert_eq!(checkpoint.http_status, Some(expected_status));
            assert_eq!(checkpoint_json["http_status"], expected_status);
            assert!(checkpoint_json["http_status"].is_number());
            assert_eq!(checkpoint.dimension, None);
            assert_eq!(fake.generation_calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                fake.embedding_calls.load(Ordering::SeqCst),
                if generation_status.is_none() { 1 } else { 0 }
            );
            assert_eq!(budget.used(), expected_requests);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prepared_checkpoint_missing_http_status_is_rejected_before_provider_calls() {
        let dir = tempdir().unwrap();
        let cfg = config(dir.path());
        let budget = Arc::new(ProbeRequestBudget::new());
        let fake = HttpStatusFake {
            generation_calls: AtomicU32::new(0),
            embedding_calls: AtomicU32::new(0),
            generation_status: None,
            embedding_status: None,
            budget: budget.clone(),
        };
        prepare_fake_probe(&cfg).await;

        let path = checkpoint_path(&cfg);
        let mut checkpoint_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        checkpoint_json
            .as_object_mut()
            .unwrap()
            .remove("http_status");
        fs::write(&path, serde_json::to_vec(&checkpoint_json).unwrap()).unwrap();

        assert_eq!(
            run_capability_probe(&cfg, &fake, budget.clone())
                .await
                .unwrap_err(),
            ProbeError::Checkpoint
        );
        assert_eq!(fake.generation_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fake.embedding_calls.load(Ordering::SeqCst), 0);
        assert_eq!(budget.used(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn generation_requires_exact_ok_true_before_embedding() {
        assert_eq!(
            generation_probe_schema()["properties"]["ok"]["enum"],
            json!([true])
        );
        for invalid_value in [
            json!({"ok": false}),
            json!({}),
            json!({"ok": "true"}),
            json!({"ok": true, "extra": 1}),
        ] {
            let dir = tempdir().unwrap();
            let cfg = config(dir.path());
            let budget = Arc::new(ProbeRequestBudget::new());
            let fake = Fake {
                generation_calls: AtomicU32::new(0),
                embedding_calls: AtomicU32::new(0),
                fail_generation: false,
                fail_embedding: false,
                embedding_dimension: 3,
                generation_value: invalid_value,
                budget: budget.clone(),
            };

            prepare_fake_probe(&cfg).await;
            assert_eq!(
                run_capability_probe(&cfg, &fake, budget.clone())
                    .await
                    .unwrap_err(),
                ProbeError::Provider("probe_structured_output_mismatch".to_owned())
            );
            assert_eq!(fake.generation_calls.load(Ordering::SeqCst), 1);
            assert_eq!(fake.embedding_calls.load(Ordering::SeqCst), 0);
            assert_eq!(budget.used(), 1);
            let checkpoint: ProbeCheckpoint = serde_json::from_slice(
                &fs::read(
                    Path::new(&cfg.artifact_root).join("provider-capability-probe.checkpoint.json"),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(checkpoint.stage, ProbeStage::Generation);
            assert_eq!(checkpoint.status, ProbeStatus::Failed);
            assert_eq!(checkpoint.request_count, 1);
            assert_eq!(
                checkpoint.error_code.as_deref(),
                Some("probe_structured_output_mismatch")
            );
            assert_eq!(checkpoint.http_status, None);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_probe_is_exactly_two_requests_and_cannot_be_replayed() {
        let dir = tempdir().unwrap();
        let cfg = config(dir.path());
        validate_provider_capability_probe_config(&cfg).unwrap();
        assert!(
            !Path::new(&cfg.run_root).exists(),
            "preflight created run state"
        );
        let budget = Arc::new(ProbeRequestBudget::new());
        let fake = Fake {
            generation_calls: AtomicU32::new(0),
            embedding_calls: AtomicU32::new(0),
            fail_generation: false,
            fail_embedding: false,
            embedding_dimension: 3,
            generation_value: json!({"ok": true}),
            budget: budget.clone(),
        };
        prepare_fake_probe(&cfg).await;
        let checkpoint = run_capability_probe(&cfg, &fake, budget.clone())
            .await
            .unwrap();
        assert_eq!(fake.generation_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fake.embedding_calls.load(Ordering::SeqCst), 1);
        assert_eq!(budget.used(), 2);
        assert_eq!(checkpoint.request_count, 2);
        assert_eq!(checkpoint.dimension, Some(3));
        assert_eq!(checkpoint.http_status, None);
        assert_eq!(
            checkpoint.dimension_status,
            Some(ProbeDimensionStatus::Matched)
        );
        assert_eq!(checkpoint.status, ProbeStatus::Succeeded);
        assert_eq!(
            run_capability_probe(&cfg, &fake, budget).await.unwrap_err(),
            ProbeError::Checkpoint
        );
        assert_eq!(fake.generation_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fake.embedding_calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn probe_generation_wire_payload_disables_thinking_and_requests_256_tokens() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/chat/completions", post(capture_probe_generation))
            .route("/v1/embeddings", post(return_probe_embedding))
            .with_state(captured.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = tempdir().unwrap();
        let mut cfg = config(dir.path());
        cfg.generation.kind = ProviderKind::XiaomiMimo;
        cfg.generation.base_url = format!("http://{address}/v1/");
        cfg.generation.capabilities.max_output_tokens = Some(PROBE_GENERATION_TOKEN_LIMIT);
        cfg.embedding.base_url = format!("http://{address}/v1/");
        let (boundary, budget) = prepare_provider_capability_probe(&cfg).await.unwrap();
        let checkpoint = run_capability_probe(&cfg, &boundary, budget.clone())
            .await
            .unwrap();

        assert_eq!(checkpoint.status, ProbeStatus::Succeeded);
        assert_eq!(checkpoint.request_count, 2);
        assert_eq!(budget.used(), 2);
        let requests = captured.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]["max_completion_tokens"],
            PROBE_GENERATION_TOKEN_LIMIT
        );
        assert_eq!(requests[0]["thinking"]["type"], "disabled");
        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prepared_config_drift_fails_during_provisioning_without_provider_calls() {
        let dir = tempdir().unwrap();
        let cfg = config(dir.path());
        let budget = Arc::new(ProbeRequestBudget::new());
        let fake = Fake {
            generation_calls: AtomicU32::new(0),
            embedding_calls: AtomicU32::new(0),
            fail_generation: false,
            fail_embedding: false,
            embedding_dimension: 3,
            generation_value: json!({"ok": true}),
            budget: budget.clone(),
        };
        prepare_fake_probe(&cfg).await;
        let mut changed = cfg.clone();
        changed.generation.model_id = "changed-after-prepare".to_owned();

        assert_eq!(
            run_capability_probe(&changed, &fake, budget.clone())
                .await
                .unwrap_err(),
            ProbeError::Config
        );
        assert_eq!(fake.generation_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fake.embedding_calls.load(Ordering::SeqCst), 0);
        assert_eq!(budget.used(), 0);
        let checkpoint: ProbeCheckpoint =
            serde_json::from_slice(&fs::read(checkpoint_path(&cfg)).unwrap()).unwrap();
        assert_eq!(checkpoint.stage, ProbeStage::Provisioning);
        assert_eq!(checkpoint.status, ProbeStatus::Failed);
        assert_eq!(checkpoint.request_count, 0);
        assert_eq!(
            checkpoint.error_code.as_deref(),
            Some("probe_config_invalid")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unexpected_prepared_root_entry_is_rejected_and_checkpoint_fails_closed() {
        let dir = tempdir().unwrap();
        let cfg = config(dir.path());
        let budget = Arc::new(ProbeRequestBudget::new());
        let fake = Fake {
            generation_calls: AtomicU32::new(0),
            embedding_calls: AtomicU32::new(0),
            fail_generation: false,
            fail_embedding: false,
            embedding_dimension: 3,
            generation_value: json!({"ok": true}),
            budget: budget.clone(),
        };
        prepare_fake_probe(&cfg).await;
        let unexpected = Path::new(&cfg.run_root).join("unexpected");
        fs::write(unexpected, b"not part of a prepared probe").unwrap();

        assert_eq!(
            run_capability_probe(&cfg, &fake, budget.clone())
                .await
                .unwrap_err(),
            ProbeError::UnsafeRoots
        );
        assert_eq!(fake.generation_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fake.embedding_calls.load(Ordering::SeqCst), 0);
        assert_eq!(budget.used(), 0);
        let checkpoint: ProbeCheckpoint =
            serde_json::from_slice(&fs::read(checkpoint_path(&cfg)).unwrap()).unwrap();
        assert_eq!(checkpoint.stage, ProbeStage::Provisioning);
        assert_eq!(checkpoint.status, ProbeStatus::Failed);
        assert_eq!(
            checkpoint.error_code.as_deref(),
            Some("probe_isolation_failed")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unrelated_existing_key_is_rejected_and_never_reaches_provider() {
        let dir = tempdir().unwrap();
        let cfg = config(dir.path());
        let budget = Arc::new(ProbeRequestBudget::new());
        let fake = Fake {
            generation_calls: AtomicU32::new(0),
            embedding_calls: AtomicU32::new(0),
            fail_generation: false,
            fail_embedding: false,
            embedding_dimension: 3,
            generation_value: json!({"ok": true}),
            budget: budget.clone(),
        };
        prepare_fake_probe(&cfg).await;
        fs::write(&cfg.master_key_path, [0_u8; 32]).unwrap();

        assert_eq!(
            run_capability_probe(&cfg, &fake, budget.clone())
                .await
                .unwrap_err(),
            ProbeError::Checkpoint
        );
        assert_eq!(fake.generation_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fake.embedding_calls.load(Ordering::SeqCst), 0);
        assert_eq!(budget.used(), 0);
        let checkpoint: ProbeCheckpoint =
            serde_json::from_slice(&fs::read(checkpoint_path(&cfg)).unwrap()).unwrap();
        assert_eq!(checkpoint.stage, ProbeStage::Provisioning);
        assert_eq!(checkpoint.status, ProbeStatus::Failed);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn discovery_records_positive_dimension_without_configuring_model_capability() {
        let dir = tempdir().unwrap();
        let mut cfg = config(dir.path());
        cfg.embedding_expected_dimension = ExpectedEmbeddingDimension::Discover;
        cfg.embedding.capabilities.dimension = None;
        let budget = Arc::new(ProbeRequestBudget::new());
        let fake = Fake {
            generation_calls: AtomicU32::new(0),
            embedding_calls: AtomicU32::new(0),
            fail_generation: false,
            fail_embedding: false,
            embedding_dimension: 768,
            generation_value: json!({"ok": true}),
            budget: budget.clone(),
        };

        prepare_fake_probe(&cfg).await;
        let checkpoint = run_capability_probe(&cfg, &fake, budget.clone())
            .await
            .unwrap();

        assert_eq!(checkpoint.dimension, Some(768));
        assert_eq!(
            checkpoint.dimension_status,
            Some(ProbeDimensionStatus::Discovered)
        );
        assert_eq!(checkpoint.request_count, 2);
        assert_eq!(budget.used(), 2);
        assert_eq!(cfg.embedding.capabilities.dimension, None);
        assert_eq!(fake.generation_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fake.embedding_calls.load(Ordering::SeqCst), 1);
        let checkpoint_path =
            Path::new(&cfg.artifact_root).join("provider-capability-probe.checkpoint.json");
        let checkpoint_json: serde_json::Value =
            serde_json::from_slice(&fs::read(checkpoint_path).unwrap()).unwrap();
        assert_eq!(checkpoint_json["dimension_status"], "discovered");
        assert_eq!(checkpoint_json["dimension"], 768);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn discovered_zero_dimension_and_embedding_error_stop_with_failed_checkpoint() {
        for (
            returned_dimension,
            fail_embedding,
            expected_code,
            expected_dimension,
            expected_status,
        ) in [
            (
                0,
                false,
                "embedding_dimension_invalid",
                Some(0),
                Some(ProbeDimensionStatus::Invalid),
            ),
            (0, true, "provider_response_invalid", None, None),
        ] {
            let dir = tempdir().unwrap();
            let mut cfg = config(dir.path());
            cfg.embedding_expected_dimension = ExpectedEmbeddingDimension::Discover;
            cfg.embedding.capabilities.dimension = None;
            let budget = Arc::new(ProbeRequestBudget::new());
            let fake = Fake {
                generation_calls: AtomicU32::new(0),
                embedding_calls: AtomicU32::new(0),
                fail_generation: false,
                fail_embedding,
                embedding_dimension: returned_dimension,
                generation_value: json!({"ok": true}),
                budget: budget.clone(),
            };

            prepare_fake_probe(&cfg).await;
            assert!(
                run_capability_probe(&cfg, &fake, budget.clone())
                    .await
                    .is_err()
            );
            assert_eq!(fake.generation_calls.load(Ordering::SeqCst), 1);
            assert_eq!(fake.embedding_calls.load(Ordering::SeqCst), 1);
            assert_eq!(budget.used(), 2);
            let checkpoint: ProbeCheckpoint = serde_json::from_slice(
                &fs::read(
                    Path::new(&cfg.artifact_root).join("provider-capability-probe.checkpoint.json"),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(checkpoint.stage, ProbeStage::Embedding);
            assert_eq!(checkpoint.status, ProbeStatus::Failed);
            assert_eq!(checkpoint.error_code.as_deref(), Some(expected_code));
            assert_eq!(checkpoint.http_status, None);
            assert_eq!(checkpoint.dimension, expected_dimension);
            assert_eq!(checkpoint.dimension_status, expected_status);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn known_dimension_mismatch_stops_and_records_actual_dimension() {
        let dir = tempdir().unwrap();
        let cfg = config(dir.path());
        let budget = Arc::new(ProbeRequestBudget::new());
        let fake = Fake {
            generation_calls: AtomicU32::new(0),
            embedding_calls: AtomicU32::new(0),
            fail_generation: false,
            fail_embedding: false,
            embedding_dimension: 4,
            generation_value: json!({"ok": true}),
            budget: budget.clone(),
        };

        prepare_fake_probe(&cfg).await;
        assert_eq!(
            run_capability_probe(&cfg, &fake, budget.clone())
                .await
                .unwrap_err(),
            ProbeError::Provider("embedding_dimension_mismatch".to_owned())
        );
        assert_eq!(fake.generation_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fake.embedding_calls.load(Ordering::SeqCst), 1);
        assert_eq!(budget.used(), 2);
        let checkpoint: ProbeCheckpoint = serde_json::from_slice(
            &fs::read(
                Path::new(&cfg.artifact_root).join("provider-capability-probe.checkpoint.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(checkpoint.stage, ProbeStage::Embedding);
        assert_eq!(checkpoint.status, ProbeStatus::Failed);
        assert_eq!(checkpoint.dimension, Some(4));
        assert_eq!(
            checkpoint.dimension_status,
            Some(ProbeDimensionStatus::Mismatch)
        );
        assert_eq!(budget.used(), 2);
    }

    #[test]
    fn expected_dimension_mode_requires_explicit_consistent_model_assertion() {
        assert_eq!(
            serde_json::to_value(ExpectedEmbeddingDimension::Discover).unwrap(),
            json!({"mode": "discover"})
        );
        assert_eq!(
            serde_json::to_value(ExpectedEmbeddingDimension::Known { dimension: 2048 }).unwrap(),
            json!({"mode": "known", "dimension": 2048})
        );

        let dir = tempdir().unwrap();
        let mut cfg = config(dir.path());
        cfg.embedding_expected_dimension = ExpectedEmbeddingDimension::Discover;
        assert_eq!(
            validate_provider_capability_probe_config(&cfg),
            Err(ProbeError::Config)
        );
        cfg.embedding.capabilities.dimension = None;
        validate_provider_capability_probe_config(&cfg).unwrap();

        cfg.embedding_expected_dimension = ExpectedEmbeddingDimension::Known { dimension: 3 };
        cfg.embedding.capabilities.dimension = Some(4);
        assert_eq!(
            validate_provider_capability_probe_config(&cfg),
            Err(ProbeError::Config)
        );
        cfg.embedding.capabilities.dimension = Some(3);
        cfg.embedding_expected_dimension = ExpectedEmbeddingDimension::Known { dimension: 0 };
        assert_eq!(
            validate_provider_capability_probe_config(&cfg),
            Err(ProbeError::Config)
        );
    }

    #[test]
    fn generation_capability_cannot_silently_clamp_probe_token_limit() {
        let dir = tempdir().unwrap();
        let mut cfg = config(dir.path());
        cfg.generation.capabilities.max_output_tokens = Some(PROBE_GENERATION_TOKEN_LIMIT - 1);
        assert_eq!(validate_probe_config_fields(&cfg), Err(ProbeError::Config));
        cfg.generation.capabilities.max_output_tokens = Some(PROBE_GENERATION_TOKEN_LIMIT);
        assert!(validate_probe_config_fields(&cfg).is_ok());
    }

    #[tokio::test]
    async fn request_budget_is_a_non_configurable_hard_cap() {
        let budget = ProbeRequestBudget::new();
        budget.reserve(1).await.unwrap();
        budget.reserve(1).await.unwrap();
        assert!(budget.reserve(1).await.is_err());
        assert_eq!(budget.used(), 2);
    }

    #[test]
    fn preflight_rejects_default_data_and_root_overlap_without_creating_paths() {
        let dir = tempdir().unwrap();
        let mut cfg = config(dir.path());
        cfg.state_root = Path::new(&cfg.run_root).join("state").display().to_string();
        cfg.artifact_root = cfg.state_root.clone();
        assert_eq!(
            validate_provider_capability_probe_config(&cfg),
            Err(ProbeError::UnsafeRoots)
        );
        cfg.artifact_root = Path::new(&cfg.run_root)
            .join("artifacts")
            .display()
            .to_string();
        cfg.state_root = dir.path().join("data/state").display().to_string();
        assert!(validate_provider_capability_probe_config(&cfg).is_err());
        assert!(!Path::new(&cfg.run_root).exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn insecure_run_root_is_rejected_before_state_or_key_creation() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let cfg = config(dir.path());
        fs::create_dir(&cfg.run_root).unwrap();
        fs::set_permissions(&cfg.run_root, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            validate_provider_capability_probe_config(&cfg),
            Err(ProbeError::UnsafeRoots)
        );
        assert!(matches!(
            prepare_provider_capability_probe(&cfg).await,
            Err(ProbeError::UnsafeRoots)
        ));
        assert!(!Path::new(&cfg.state_root).exists());
        assert!(!Path::new(&cfg.master_key_path).exists());
    }

    #[cfg(unix)]
    #[test]
    fn preflight_rejects_insecure_destination_key_parent_and_existing_key() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let cfg = config(dir.path());
        fs::create_dir(&cfg.run_root).unwrap();
        fs::set_permissions(&cfg.run_root, fs::Permissions::from_mode(0o700)).unwrap();
        let key_parent = Path::new(&cfg.master_key_path).parent().unwrap();
        fs::create_dir(key_parent).unwrap();
        fs::set_permissions(key_parent, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            validate_provider_capability_probe_config(&cfg),
            Err(ProbeError::UnsafeRoots)
        );

        fs::set_permissions(key_parent, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(&cfg.master_key_path, [0_u8; 32]).unwrap();
        fs::set_permissions(&cfg.master_key_path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            validate_provider_capability_probe_config(&cfg),
            Err(ProbeError::UnsafeRoots)
        );
    }

    #[cfg(unix)]
    #[test]
    fn preflight_rejects_insecure_source_secret_file_or_parent() {
        use std::os::unix::fs::PermissionsExt;

        for (parent_mode, key_mode) in [
            (0o755, 0o600),
            (0o720, 0o600),
            (0o700, 0o644),
            (0o700, 0o620),
        ] {
            let dir = tempdir().unwrap();
            let source_root = fs::canonicalize(dir.path())
                .unwrap()
                .join("source-installation");
            fs::create_dir(&source_root).unwrap();
            fs::set_permissions(&source_root, fs::Permissions::from_mode(parent_mode)).unwrap();
            let source_db = source_root.join("state.sqlite3");
            fs::write(&source_db, b"synthetic source state path").unwrap();
            let source_key = source_root.join("master-key");
            fs::write(&source_key, [0_u8; 32]).unwrap();
            fs::set_permissions(&source_key, fs::Permissions::from_mode(key_mode)).unwrap();

            let mut cfg = config(dir.path());
            cfg.credential_provisioning = Some(ProbeCredentialProvisioning {
                generation: Some(ProbeSourceCredential {
                    source_database_path: source_db.display().to_string(),
                    source_master_key_path: source_key.display().to_string(),
                    source_provider_id: ProviderId::new().to_string(),
                }),
                embedding: None,
            });
            assert_eq!(
                validate_provider_capability_probe_config(&cfg),
                Err(ProbeError::UnsafeRoots)
            );
            assert!(!Path::new(&cfg.run_root).exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn preflight_rejects_symlinked_run_root_and_source_secret_parent() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let dir = tempdir().unwrap();
        let real_run = fs::canonicalize(dir.path()).unwrap().join("real-run");
        fs::create_dir(&real_run).unwrap();
        fs::set_permissions(&real_run, fs::Permissions::from_mode(0o700)).unwrap();
        let run_alias = fs::canonicalize(dir.path()).unwrap().join("run-alias");
        symlink(&real_run, &run_alias).unwrap();
        let mut cfg = config(dir.path());
        cfg.run_root = run_alias.display().to_string();
        cfg.state_root = run_alias.join("state").display().to_string();
        cfg.vault_root = run_alias.join("vaults/probe").display().to_string();
        cfg.history_root = run_alias.join("history").display().to_string();
        cfg.artifact_root = run_alias.join("artifacts").display().to_string();
        cfg.master_key_path = run_alias.join("keys/master").display().to_string();
        assert_eq!(
            validate_provider_capability_probe_config(&cfg),
            Err(ProbeError::UnsafeRoots)
        );

        let source_parent = fs::canonicalize(dir.path()).unwrap().join("source-parent");
        fs::create_dir(&source_parent).unwrap();
        fs::set_permissions(&source_parent, fs::Permissions::from_mode(0o700)).unwrap();
        let source_db = source_parent.join("state.sqlite3");
        let source_key = source_parent.join("master-key");
        fs::write(&source_db, b"synthetic source state path").unwrap();
        fs::write(&source_key, [0_u8; 32]).unwrap();
        fs::set_permissions(&source_key, fs::Permissions::from_mode(0o600)).unwrap();
        let source_alias = fs::canonicalize(dir.path()).unwrap().join("source-alias");
        symlink(&source_parent, &source_alias).unwrap();

        let mut cfg = config(dir.path());
        cfg.credential_provisioning = Some(ProbeCredentialProvisioning {
            generation: Some(ProbeSourceCredential {
                source_database_path: source_db.display().to_string(),
                source_master_key_path: source_alias.join("master-key").display().to_string(),
                source_provider_id: ProviderId::new().to_string(),
            }),
            embedding: None,
        });
        assert_eq!(
            validate_provider_capability_probe_config(&cfg),
            Err(ProbeError::UnsafeRoots)
        );
        assert!(!Path::new(&cfg.run_root).exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn provisioning_reencrypts_source_secret_into_fresh_destination_key() {
        use mcp_vault_auth::{AuthService, SecretString, load_or_create_master_key};
        use mcp_vault_state::StateStore;
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let source_root = fs::canonicalize(dir.path())
            .unwrap()
            .join("source-installation");
        fs::create_dir_all(&source_root).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&source_root, fs::Permissions::from_mode(0o700)).unwrap();
        let source_db = source_root.join("state.sqlite3");
        let source_key_path = source_root.join("master-key");
        let source_state =
            StateStore::connect_and_migrate(&format!("sqlite://{}", source_db.display()))
                .await
                .unwrap();
        let source_key = load_or_create_master_key(&source_key_path).await.unwrap();
        #[cfg(unix)]
        fs::set_permissions(&source_key_path, fs::Permissions::from_mode(0o600)).unwrap();
        let source_auth = AuthService::new(source_state.auth(), source_key.clone());
        let source_provider = ProviderService::new(source_state.clone(), source_auth.clone())
            .create_provider(ProviderInput {
                name: "source credential".to_owned(),
                kind: ProviderKind::OpenAiCompatible,
                base_url: "http://127.0.0.1:9/v1/".parse().unwrap(),
                settings: ProviderSettings::default(),
                enabled: true,
                secret: Some(SecretString::new("synthetic-test-secret")),
            })
            .await
            .unwrap();
        let source_secret_id = source_provider.secret_id.unwrap();
        let source_cipher = source_state
            .auth()
            .get_secret(source_secret_id)
            .await
            .unwrap()
            .unwrap();
        let source_secret_count = source_state.auth().count_encrypted_secrets().await.unwrap();

        let mut cfg = config(dir.path());
        let source = ProbeSourceCredential {
            source_database_path: source_db.display().to_string(),
            source_master_key_path: source_key_path.display().to_string(),
            source_provider_id: source_provider.id.to_string(),
        };
        cfg.credential_provisioning = Some(ProbeCredentialProvisioning {
            generation: Some(source.clone()),
            embedding: Some(source),
        });
        let (_boundary, _budget) = prepare_provider_capability_probe(&cfg).await.unwrap();

        let destination_db = Path::new(&cfg.state_root).join("state.sqlite3");
        let destination_state =
            StateStore::connect_read_only(&format!("sqlite://{}", destination_db.display()))
                .await
                .unwrap();
        let destination_providers = destination_state
            .providers()
            .list_providers(10)
            .await
            .unwrap();
        assert_eq!(destination_providers.len(), 2);
        let destination_key = load_or_create_master_key(Path::new(&cfg.master_key_path))
            .await
            .unwrap();
        let destination_auth = AuthService::new(destination_state.auth(), destination_key);
        let destination_wrong_key_state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let destination_wrong_key_auth =
            AuthService::new(destination_wrong_key_state.auth(), source_key);
        let mut destination_ciphertexts = Vec::new();
        for provider in destination_providers {
            assert_eq!(
                provider
                    .settings
                    .get("max_retries")
                    .and_then(serde_json::Value::as_u64),
                Some(0)
            );
            let secret_id = provider.secret_id.unwrap();
            let encrypted = destination_state
                .auth()
                .get_secret(secret_id)
                .await
                .unwrap()
                .unwrap();
            #[cfg(unix)]
            {
                assert_eq!(
                    fs::metadata(&cfg.run_root).unwrap().permissions().mode() & 0o777,
                    0o700
                );
                for directory in [
                    Path::new(&cfg.state_root),
                    Path::new(&cfg.vault_root),
                    Path::new(&cfg.history_root),
                    Path::new(&cfg.artifact_root),
                    Path::new(&cfg.master_key_path).parent().unwrap(),
                ] {
                    assert_eq!(
                        fs::metadata(directory).unwrap().permissions().mode() & 0o777,
                        0o700
                    );
                }
                assert_eq!(
                    fs::metadata(&cfg.master_key_path)
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
                assert_eq!(
                    fs::metadata(
                        Path::new(&cfg.artifact_root)
                            .join("provider-capability-probe.checkpoint.json")
                    )
                    .unwrap()
                    .permissions()
                    .mode()
                        & 0o777,
                    0o600
                );
            }
            assert_ne!(encrypted.ciphertext, source_cipher.ciphertext);
            assert_ne!(encrypted.nonce, source_cipher.nonce);
            destination_ciphertexts.push(encrypted.ciphertext.clone());
            let plaintext = destination_auth
                .read_installation_secret(
                    secret_id,
                    PROVIDER_SECRET_PURPOSE,
                    PROVIDER_SECRET_OWNER,
                    Some(&provider.id.to_string()),
                )
                .await
                .unwrap();
            assert_eq!(plaintext.expose_secret(), "synthetic-test-secret");

            // Copy only to an isolated test State to prove the source key
            // cannot decrypt destination ciphertext with matching owner AAD.
            let transplanted_id = mcp_vault_domain::SecretId::new();
            destination_wrong_key_state
                .auth()
                .insert_secret(
                    transplanted_id,
                    &encrypted.purpose,
                    &encrypted.owner_type,
                    encrypted.owner_id.as_deref(),
                    encrypted.key_version,
                    &encrypted.nonce,
                    &encrypted.ciphertext,
                    encrypted.hint.as_deref(),
                )
                .await
                .unwrap();
            assert!(
                destination_wrong_key_auth
                    .read_installation_secret(
                        transplanted_id,
                        PROVIDER_SECRET_PURPOSE,
                        PROVIDER_SECRET_OWNER,
                        Some(&provider.id.to_string()),
                    )
                    .await
                    .is_err()
            );
            assert!(
                destination_auth
                    .read_installation_secret(
                        secret_id,
                        PROVIDER_SECRET_PURPOSE,
                        PROVIDER_SECRET_OWNER,
                        Some(&source_provider.id.to_string()),
                    )
                    .await
                    .is_err()
            );
        }
        assert_ne!(destination_ciphertexts[0], destination_ciphertexts[1]);
        assert_eq!(
            source_state.auth().count_encrypted_secrets().await.unwrap(),
            source_secret_count,
            "source State must remain read-only during provisioning"
        );
        let vault = destination_state
            .vaults()
            .find_by_slug(&VaultSlug::new("probe").unwrap())
            .await
            .unwrap()
            .unwrap();
        let models = destination_state
            .providers()
            .list_models(None, 10)
            .await
            .unwrap();
        let embedding_model = models
            .into_iter()
            .find(|model| model.external_model_id == cfg.embedding.model_id)
            .unwrap();
        assert!(
            destination_state
                .providers()
                .list_embeddings(
                    &vault.context().unwrap(),
                    embedding_model.id,
                    "probe",
                    10,
                    0,
                )
                .await
                .unwrap()
                .is_empty()
        );
    }
}
