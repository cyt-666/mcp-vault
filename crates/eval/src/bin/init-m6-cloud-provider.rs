//! Create fresh M6 Provider state from a Codex Network secret placeholder.
//! This entry point never discovers models or sends Provider requests.

use std::{env, error::Error, path::PathBuf};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
const INIT_FLAG: &str = "--initialize-authorized-m6-provider";
#[cfg(unix)]
const SECRET_ENV: &str = "MIMO_API_KEY";

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    if args.next().as_deref() != Some(INIT_FLAG) {
        return Err("usage: init-m6-cloud-provider --initialize-authorized-m6-provider NEW_ABSOLUTE_ROOT; requires MIMO_API_KEY configured as a Codex Network secret".into());
    }
    let root = args
        .next()
        .map(PathBuf::from)
        .ok_or("new root is required")?;
    if args.next().is_some() {
        return Err("unexpected initialization arguments".into());
    }
    initialize(root).await
}

#[cfg(not(unix))]
async fn initialize(_root: PathBuf) -> Result<()> {
    Err("private cloud Provider initialization requires Unix permissions".into())
}

#[cfg(unix)]
async fn initialize(root: PathBuf) -> Result<()> {
    use mcp_vault_auth::{AuthService, SecretString, load_or_create_master_key};
    use mcp_vault_domain::{Revision, VaultContext, VaultId, VaultSlug};
    use mcp_vault_providers::{
        ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderMode,
        ProviderService, ProviderSettings,
    };
    use mcp_vault_state::{StateStore, VaultStatus};
    use serde_json::json;
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, PermissionsExt},
        path::Component,
    };

    if !root.is_absolute()
        || root
            .to_str()
            .is_none_or(|path| path.contains(['%', '?', '#']) || path.chars().any(char::is_control))
        || root
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || root.file_name().is_none()
    {
        return Err("initialization root must be a new absolute normalized directory without SQLite URI metacharacters".into());
    }
    let parent = root.parent().ok_or("initialization parent is missing")?;
    if fs::canonicalize(parent)? != parent {
        return Err("initialization parent must exist without symlinks".into());
    }
    if fs::symlink_metadata(&root).is_ok() {
        return Err("initialization root already exists; refusing to change existing state".into());
    }
    // Never include VarError's Debug representation: invalid Unicode can carry
    // the value. The raw credential belongs only in Personal vault, not here.
    let secret = SecretString::new(env::var(SECRET_ENV).map_err(|_| {
        "MIMO_API_KEY is unavailable; request it as a Network secret in the cloud environment and start a new task"
    })?);
    if secret.expose_secret().trim().is_empty()
        || secret.as_bytes().iter().any(|byte| byte.is_ascii_control())
    {
        return Err("MIMO_API_KEY must contain a nonempty HTTP header value".into());
    }

    // create() is exclusive. A fresh root prevents overwriting any existing
    // Provider, run, or installation key; all descendants remain private.
    fs::DirBuilder::new().mode(0o700).create(&root)?;
    let state_root = root.join("state");
    let source_root = root.join("source");
    for path in [&state_root, &source_root] {
        fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    let database_path = state_root.join("state.sqlite3");
    let master_key_path = state_root.join("master.key");
    let state =
        StateStore::connect_and_migrate(&format!("sqlite://{}", database_path.display())).await?;
    fs::set_permissions(&database_path, fs::Permissions::from_mode(0o600))?;
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("m6-cloud-config")?,
        source_root,
        Revision::ZERO,
    )?;
    state
        .vaults()
        .insert(&context, "M6 cloud configuration", VaultStatus::Active)
        .await?;
    let keys = load_or_create_master_key(&master_key_path).await?;
    fs::set_permissions(&master_key_path, fs::Permissions::from_mode(0o600))?;
    let auth = AuthService::new(state.auth(), keys);
    let providers = ProviderService::new(state, auth);
    providers
        .set_provider_mode(&context, ProviderMode::Enabled, None)
        .await?;
    let provider = providers
        .create_provider(ProviderInput {
            name: "M6 official MiMo cloud configuration".into(),
            kind: ProviderKind::XiaomiMimo,
            base_url: url::Url::parse("https://api.xiaomimimo.com/v1/")?,
            settings: ProviderSettings {
                timeout_ms: 600_000,
                connect_timeout_ms: 5_000,
                max_retries: 0,
                max_concurrency: 1,
                ..Default::default()
            },
            enabled: true,
            secret: Some(secret),
        })
        .await?;
    let model = providers
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "mimo-v2.6-flash".into(),
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
        .await?;
    providers
        .bind_model(
            Some(&context),
            "memory_extraction",
            model.id,
            json!({}),
            None,
        )
        .await?;
    println!(
        "{}",
        json!({
            "initialized": true,
            "source_database_path": database_path,
            "source_master_key_path": master_key_path,
            "source_vault_slug": context.slug().as_str(),
            "provider_type": "xiaomi_mimo",
            "base_url": "https://api.xiaomimimo.com/v1/",
            "external_model_id": "mimo-v2.6-flash",
            "generation_token_limit": 32_768,
            "provider_timeout_seconds": 600,
            "connect_timeout_ms": 5_000,
            "max_retries": 0,
            "max_concurrency": 1,
            "real_provider_requests_started": 0,
            "authentication_status": "not_tested",
        })
    );
    Ok(())
}
