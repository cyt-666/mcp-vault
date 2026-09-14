//! Explicitly authorized live-model validation against an isolated source copy.
//! Never imports old memory records and never prints credentials or source bodies.
use mcp_vault_auth::{AuthService, MasterKeyRing, SecretString, load_or_create_master_key};
use mcp_vault_core::VaultCore;
use mcp_vault_domain::{Actor, Revision, SourcePlane, VaultContext, VaultId, VaultPath, VaultSlug};
use mcp_vault_memory::{ExtractionPolicy, MemoryService};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderMode,
    ProviderService, ProviderSettings,
};
use mcp_vault_state::{ModelRecord, StateStore, VaultStatus};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    error::Error,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.0.id().to_string()])
            .status();
        let _ = self.0.wait();
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_selection_manifest(manifest: &Value) -> Result<&[Value]> {
    let sources = manifest["sources"]
        .as_array()
        .ok_or("frozen manifest sources must be an array")?;
    if sources.is_empty() {
        return Err("frozen manifest sources must not be empty".into());
    }
    let source_count = manifest["source_count"]
        .as_u64()
        .ok_or("frozen manifest source_count must be an integer")?;
    if source_count != sources.len() as u64 {
        return Err("frozen manifest source_count does not match sources".into());
    }
    let mut paths = HashSet::new();
    for source in sources {
        let path = source["path"]
            .as_str()
            .ok_or("frozen manifest source path must be a string")?;
        VaultPath::parse(path).map_err(|_| "frozen manifest source path is invalid")?;
        if !path.to_ascii_lowercase().ends_with(".md") {
            return Err("frozen manifest source must be Markdown".into());
        }
        if !paths.insert(path.to_owned()) {
            return Err("frozen manifest contains duplicate source paths".into());
        }
        let sha256 = source["sha256"]
            .as_str()
            .ok_or("frozen manifest source sha256 must be a string")?;
        if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("frozen manifest source sha256 is invalid".into());
        }
    }
    Ok(sources)
}
fn save(path: &Path, value: &Value) -> Result<()> {
    std::fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
async fn api(
    client: &reqwest::Client,
    origin: &str,
    route: &str,
    cookie: &str,
    csrf: &str,
    method: &str,
    body: Option<Value>,
) -> Result<Value> {
    let mut request = client
        .request(
            method.parse()?,
            format!("{origin}/api/v1/vaults/default{route}"),
        )
        .header("Cookie", cookie);
    if method != "GET" {
        request = request
            .header("Origin", origin)
            .header("x-csrf-token", csrf);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let value: Value = response.json().await?;
    if !status.is_success() || value.get("error").is_some_and(|error| !error.is_null()) {
        return Err(format!("Admin {} {}", status.as_u16(), value["error"]["code"]).into());
    }
    Ok(value["data"].clone())
}
async fn clone_model(
    source: &StateStore,
    source_auth: &AuthService,
    source_context: &VaultContext,
    target: &ProviderService,
    target_context: &VaultContext,
    source_role: &str,
    target_roles: &[&str],
) -> Result<ModelRecord> {
    let binding = source
        .providers()
        .resolve_binding(source_context, source_role)
        .await?
        .ok_or("source model binding missing")?;
    let model = source
        .providers()
        .get_model(binding.model_id)
        .await?
        .ok_or("source model missing")?;
    let provider = source
        .providers()
        .get_provider(model.provider_id)
        .await?
        .ok_or("source provider missing")?;
    let secret = match provider.secret_id {
        Some(id) => Some(
            source_auth
                .read_installation_secret(
                    id,
                    "provider-api-key",
                    "provider",
                    Some(&provider.id.to_string()),
                )
                .await?,
        ),
        None => None,
    };
    let created = target
        .create_provider(ProviderInput {
            name: format!("Authorized v3 {source_role}"),
            kind: ProviderKind::try_from(provider.provider_type.as_str())?,
            base_url: url::Url::parse(&provider.base_url)?,
            settings: ProviderSettings::from_json(&provider.settings)?,
            enabled: true,
            secret,
        })
        .await?;
    let created = target
        .register_model(ModelInput {
            provider_id: created.id,
            external_model_id: model.external_model_id,
            capabilities: ModelCapabilities::from_json(&model.capabilities)?,
            settings: ModelSettings::from_json(&model.settings)?,
            enabled: true,
        })
        .await?;
    for role in target_roles {
        target
            .bind_model(Some(target_context), role, created.id, json!({}), None)
            .await?;
    }
    Ok(created)
}

/// Run the source-unit selection path without starting HTTP or background workers.
///
/// This mode intentionally clones only `memory_extraction`. It is a real-model
/// selection check, not public-protocol or retrieval acceptance evidence.
async fn run_authorized_selection_only(args: &[String]) -> Result<()> {
    run_authorized_selection_only_with_mode(args, ProviderMode::RemoteAllowed).await
}

async fn run_authorized_selection_only_with_mode(
    args: &[String],
    provider_mode: ProviderMode,
) -> Result<()> {
    if args.len() != 5 {
        return Err("usage: live_memory_v3 --run-authorized-real-selection-only SOURCE_DB MASTER_KEY FROZEN_MANIFEST NEW_RUN_ROOT".into());
    }
    let manifest_path = PathBuf::from(&args[3]).canonicalize()?;
    let manifest_bytes = std::fs::read(&manifest_path)?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes)?;
    let sources = validate_selection_manifest(&manifest)?;
    let root = PathBuf::from(&args[4]);
    if root.exists() {
        return Err("selection-only run root must not already exist".into());
    }
    std::fs::create_dir_all(&root)?;
    let root = root.canonicalize()?;
    let source = StateStore::connect_read_only(&format!(
        "sqlite://{}",
        PathBuf::from(&args[1]).canonicalize()?.display()
    ))
    .await?;
    let source_auth = AuthService::new(
        source.auth(),
        MasterKeyRing::load_file(&PathBuf::from(&args[2])).await?,
    );
    let source_context = source
        .vaults()
        .list()
        .await?
        .into_iter()
        .find(|vault| vault.slug.as_str() == "default")
        .ok_or("source Vault missing")?
        .context()?;

    let database = format!("sqlite://{}/state/mcp-vault.sqlite3", root.display());
    let state = StateStore::connect_and_migrate(&database).await?;
    let auth = AuthService::new(
        state.auth(),
        load_or_create_master_key(&root.join("secrets/master-key")).await?,
    );
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("default")?,
        root.join("vaults/default"),
        Revision::ZERO,
    )?;
    state
        .vaults()
        .insert(
            &context,
            "Real source-unit selection validation",
            VaultStatus::Active,
        )
        .await?;

    let providers = ProviderService::new(state.clone(), auth);
    let model = clone_model(
        &source,
        &source_auth,
        &source_context,
        &providers,
        &context,
        "memory_extraction",
        &["memory_extraction"],
    )
    .await?;
    providers
        .set_provider_mode(&context, provider_mode, None)
        .await?;
    let memory = MemoryService::with_provider_service(state.clone(), providers);
    memory
        .set_extraction_policy(
            &context,
            ExtractionPolicy {
                enabled: true,
                ..Default::default()
            },
            None,
            None,
        )
        .await?;
    let core = VaultCore::new(
        state.clone(),
        root.join("history"),
        Default::default(),
        Default::default(),
        Default::default(),
    );

    for source in sources {
        let path = VaultPath::parse(
            source["path"]
                .as_str()
                .ok_or("frozen manifest source path must be a string")?,
        )?;
        if !path.as_str().to_ascii_lowercase().ends_with(".md") {
            return Err(format!("selection source is not Markdown: {}", path.as_str()).into());
        }
        if core.is_managed_path(&path) {
            return Err(format!("selection source is service-managed: {}", path.as_str()).into());
        }
        let bytes = std::fs::read(
            manifest_path
                .parent()
                .ok_or("manifest root missing")?
                .join("notes")
                .join(path.as_str()),
        )?;
        let content_hash = hash(&bytes);
        if source["sha256"] != content_hash {
            return Err(format!("frozen source hash mismatch for {}", path.as_str()).into());
        }
        if let Some(existing) = state.files().get_active(&context, &path).await? {
            if existing.content_hash.as_deref() != Some(content_hash.as_str()) {
                return Err(
                    format!("existing fixture source changed for {}", path.as_str()).into(),
                );
            }
        } else {
            core.create_bytes(
                &context,
                &path,
                &bytes,
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await?;
        }
    }
    source.close().await;

    let run_info = json!({
        "schema": "memory-v3-selection-only-run/v1",
        "mode": "real_selection_only_real_llm_original_sources",
        "selection_only": true,
        "source_count": sources.len(),
        "manifest_path": manifest_path,
        "manifest_sha256": hash(&manifest_bytes),
        "vault_id": context.id(),
        "model_id": model.id,
        "model": model.external_model_id,
        "embedding": "not_configured",
        "workers_started": false,
        "http_server_started": false,
        "actual_provider_requests": "not_available_from_memory_service",
        "actual_provider_tokens": "not_available_from_memory_service"
    });
    save(&root.join("run.json"), &run_info)?;

    let mut source_results = Vec::new();
    let mut application_invocations = 0_u64;
    let started = Instant::now();
    for source in sources {
        let path = VaultPath::parse(
            source["path"]
                .as_str()
                .ok_or("frozen manifest source path must be a string")?,
        )?;
        let file = state
            .files()
            .get_active(&context, &path)
            .await?
            .ok_or_else(|| format!("selection source was not imported: {}", path.as_str()))?;
        let mut invocations = 0_u32;
        let mut previous_progress = None;
        let result = loop {
            invocations = invocations.saturating_add(1);
            if invocations > 4096 {
                return Err(format!(
                    "selection did not converge for {} after 4096 invocations",
                    path.as_str()
                )
                .into());
            }
            let result = match memory.extract_note(&context, &core, &path).await {
                Ok(result) => result,
                Err(error) => {
                    let code = error.code();
                    save(
                        &root.join("selection-status.json"),
                        &json!({
                            "schema": "memory-v3-selection-only-status/v1",
                            "status": "failed",
                            "failure": {"path": path.as_str(), "code": code},
                            "application_invocations": application_invocations + u64::from(invocations),
                            "actual_provider_requests": "not_available_from_memory_service",
                            "actual_provider_tokens": "not_available_from_memory_service"
                        }),
                    )?;
                    return Err(format!("selection failed for {}: {code}", path.as_str()).into());
                }
            };
            let progress = (
                result.completed_batches,
                result.pending_batches,
                result.items_published,
                result.skipped_units,
            );
            if result.pending_batches == 0 {
                break result;
            }
            if previous_progress == Some(progress) {
                return Err(format!(
                    "selection made no progress for {} with {} pending batches",
                    path.as_str(),
                    result.pending_batches
                )
                .into());
            }
            previous_progress = Some(progress);
        };
        let completed_batches = result.completed_batches;
        let pending_batches = result.pending_batches;
        let items_published = result.items_published;
        let skipped_units = result.skipped_units;
        application_invocations += u64::from(invocations);
        let profile_hash = state
            .memory_units()
            .get_note_set_by_source(&context, file.id)
            .await?
            .map(|set| set.profile_hash);
        source_results.push(json!({
            "path": path,
            "source_file_id": file.id,
            "status": "completed",
            "invocations": invocations,
            "completed_batches": completed_batches,
            "pending_batches": pending_batches,
            "items_published": items_published,
            "skipped_units": skipped_units,
            "profile_hash": profile_hash
        }));
        save(
            &root.join("selection-status.json"),
            &json!({"schema":"memory-v3-selection-only-status/v1","sources":source_results,"application_invocations":application_invocations,"actual_provider_requests":"not_available_from_memory_service","actual_provider_tokens":"not_available_from_memory_service"}),
        )?;
    }

    let mut records = Vec::new();
    let mut offset = 0;
    loop {
        let current = memory
            .list(&context, Vec::new(), None, None, None, 200, offset)
            .await?;
        let count = current.len();
        records.extend(
            current
                .into_iter()
                .map(serde_json::to_value)
                .collect::<std::result::Result<Vec<_>, _>>()?,
        );
        if count < 200 {
            break;
        }
        offset += 200;
    }
    let status = memory.generation_status(&context).await?;
    let counts = state.memory_units().counts(&context).await?;
    save(
        &root.join("units.json"),
        &json!({"schema":"memory-v3-selection-only-units/v1","memories":records}),
    )?;
    save(
        &root.join("selection-status.json"),
        &json!({"schema":"memory-v3-selection-only-status/v1","status":status,"counts":counts,"sources":source_results,"application_invocations":application_invocations,"actual_provider_requests":"not_available_from_memory_service","actual_provider_tokens":"not_available_from_memory_service","elapsed_ms":started.elapsed().as_millis()}),
    )?;
    save(
        &root.join("run.json"),
        &json!({"schema":"memory-v3-selection-only-run/v1","mode":"real_selection_only_real_llm_original_sources","selection_only":true,"source_count":sources.len(),"manifest_path":manifest_path,"manifest_sha256":hash(&manifest_bytes),"vault_id":context.id(),"model_id":model.id,"model":model.external_model_id,"embedding":"not_configured","workers_started":false,"http_server_started":false,"selection":{"status":"completed","sources":source_results},"counts":counts,"application_invocations":application_invocations,"actual_provider_requests":"not_available_from_memory_service","actual_provider_tokens":"not_available_from_memory_service","selection_elapsed_ms":started.elapsed().as_millis()}),
    )?;
    println!(
        "{}",
        json!({"event":"real_selection_only_completed","run_root":root,"sources":sources.len(),"units":records.len(),"application_invocations":application_invocations})
    );
    state.close().await;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args
        .first()
        .is_some_and(|arg| arg == "--run-authorized-real-selection-only")
    {
        return run_authorized_selection_only(&args).await;
    }
    if args.len() != 6
        || !matches!(
            args[0].as_str(),
            "--run-authorized-real-generation" | "--resume-authorized-real-generation"
        )
    {
        return Err("usage: live_memory_v3 --run-authorized-real-generation SOURCE_DB MASTER_KEY FROZEN_MANIFEST NEW_RUN_ROOT SERVER_BINARY".into());
    }
    let manifest_path = PathBuf::from(&args[3]).canonicalize()?;
    let manifest_bytes = std::fs::read(&manifest_path)?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes)?;
    let resume = args[0] == "--resume-authorized-real-generation";
    let root = PathBuf::from(&args[4]);
    if root.exists() && !resume {
        return Err("run root must not already exist".into());
    }
    if resume {
        let prior: Value = serde_json::from_slice(&std::fs::read(root.join("run.json"))?)?;
        if prior["mode"] != "real_server_real_llm_real_embedding_original_sources"
            || prior["manifest_sha256"] != hash(&manifest_bytes)
        {
            return Err("resume requires the same isolated fixture and frozen corpus".into());
        }
    }
    std::fs::create_dir_all(&root)?;
    let root = root.canonicalize()?;
    let binary = PathBuf::from(&args[5]).canonicalize()?;
    let source = StateStore::connect_read_only(&format!(
        "sqlite://{}",
        PathBuf::from(&args[1]).canonicalize()?.display()
    ))
    .await?;
    let source_auth = AuthService::new(
        source.auth(),
        MasterKeyRing::load_file(&PathBuf::from(&args[2])).await?,
    );
    let source_context = source
        .vaults()
        .list()
        .await?
        .into_iter()
        .find(|vault| vault.slug.as_str() == "default")
        .ok_or("source Vault missing")?
        .context()?;
    let database = format!("sqlite://{}/state/mcp-vault.sqlite3", root.display());
    let state = StateStore::connect_and_migrate(&database).await?;
    let auth = AuthService::new(
        state.auth(),
        load_or_create_master_key(&root.join("secrets/master-key")).await?,
    );
    let password = SecretString::new(format!("Live-v3-{}", VaultId::new()));
    if resume {
        let user = state
            .auth()
            .find_admin_user_by_username("live-v3")
            .await?
            .ok_or("fixture admin missing")?;
        state
            .auth()
            .update_admin_password(
                user.id,
                &mcp_vault_auth::PasswordPolicy::default().hash(&password)?,
            )
            .await?;
    } else {
        auth.setup_admin("live-v3", &password).await?;
    }
    let login = auth
        .login_admin("live-v3", &password, Some("127.0.0.1"), None)
        .await?;
    let cookie = format!("mcp_vault_session={}", login.session_token.expose_secret());
    let csrf = login.csrf_token.expose_secret().to_owned();
    let context = if resume {
        state
            .vaults()
            .find_by_slug(&VaultSlug::new("default")?)
            .await?
            .ok_or("fixture Vault missing")?
            .context()?
    } else {
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("default")?,
            root.join("vaults/default"),
            Revision::ZERO,
        )?;
        state
            .vaults()
            .insert(&context, "Real source-unit validation", VaultStatus::Active)
            .await?;
        context
    };
    let providers = ProviderService::new(state.clone(), auth.clone());
    let model = if resume {
        let binding = state
            .providers()
            .resolve_binding(&context, "memory_extraction")
            .await?
            .ok_or("fixture model unbound")?;
        state
            .providers()
            .get_model(binding.model_id)
            .await?
            .ok_or("fixture model missing")?
    } else {
        clone_model(
            &source,
            &source_auth,
            &source_context,
            &providers,
            &context,
            "memory_extraction",
            &["memory_extraction"],
        )
        .await?
    };
    let embedding = if resume {
        let binding = state
            .providers()
            .resolve_binding(&context, "embedding_memory")
            .await?
            .ok_or("fixture embedding unbound")?;
        state
            .providers()
            .get_model(binding.model_id)
            .await?
            .ok_or("fixture embedding missing")?
    } else {
        clone_model(
            &source,
            &source_auth,
            &source_context,
            &providers,
            &context,
            "embedding_memory",
            &["embedding_memory", "embedding_note"],
        )
        .await?
    };
    providers
        .set_provider_mode(&context, ProviderMode::RemoteAllowed, None)
        .await?;
    let memory = MemoryService::with_provider_service(state.clone(), providers);
    memory
        .set_extraction_policy(
            &context,
            ExtractionPolicy {
                enabled: false,
                ..Default::default()
            },
            None,
            None,
        )
        .await?;
    let core = VaultCore::new(
        state.clone(),
        root.join("history"),
        Default::default(),
        Default::default(),
        Default::default(),
    );
    for source in manifest["sources"].as_array().ok_or("sources missing")? {
        let path = VaultPath::parse(source["path"].as_str().ok_or("source path missing")?)?;
        let bytes = std::fs::read(
            manifest_path
                .parent()
                .ok_or("manifest root missing")?
                .join("notes")
                .join(path.as_str()),
        )?;
        if hash(&bytes) != source["sha256"] {
            return Err("frozen source hash mismatch".into());
        }
        if let Some(existing) = state.files().get_active(&context, &path).await? {
            if existing.content_hash.as_deref() != Some(hash(&bytes).as_str()) {
                return Err("existing fixture source changed".into());
            }
        } else {
            core.create_bytes(
                &context,
                &path,
                &bytes,
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await?;
        }
    }
    let data_socket = std::net::TcpListener::bind("127.0.0.1:0")?;
    let admin_socket = std::net::TcpListener::bind("127.0.0.1:0")?;
    let data_port = data_socket.local_addr()?.port();
    let admin_port = admin_socket.local_addr()?.port();
    drop(data_socket);
    drop(admin_socket);
    let origin = format!("http://127.0.0.1:{admin_port}");
    let info = json!({"source_count":manifest["source_count"],"manifest_path":manifest_path,"manifest_sha256":hash(&manifest_bytes),"server_binary_sha256":hash(&std::fs::read(&binary)?),"admin_origin":origin,"data_origin":format!("http://127.0.0.1:{data_port}"),"vault_id":context.id(),"model_id":model.id,"model":model.external_model_id,"embedding_model_id":embedding.id,"embedding_model":embedding.external_model_id,"mode":"real_server_real_llm_real_embedding_original_sources"});
    save(&root.join("run.json"), &info)?;
    state.close().await;
    source.close().await;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("server.log"))?;
    let mut server = Server(
        Command::new(&binary)
            .env("MCP_VAULT_DATA_DIR", &root)
            .env("MCP_VAULT_DATABASE_URL", &database)
            .env("MCP_VAULT_SECRETS_DIR", root.join("secrets"))
            .env("MCP_VAULT_MASTER_KEY_FILE", root.join("secrets/master-key"))
            .env("MCP_VAULT_BACKUP_DIR", root.join("backups"))
            .env("MCP_VAULT_DATA_BIND", format!("127.0.0.1:{data_port}"))
            .env("MCP_VAULT_ADMIN_BIND", format!("127.0.0.1:{admin_port}"))
            .env("MCP_VAULT_ADMIN_ORIGINS", &origin)
            .env("MCP_VAULT_LOG_FORMAT", "json")
            .env("MCP_VAULT_RECONCILIATION_INTERVAL_SECONDS", "5")
            .env("RUST_LOG", "info,mcp_vault::memory=debug")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()?,
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()?;
    let ready = Instant::now();
    loop {
        if server.0.try_wait()?.is_some() {
            return Err("server exited; inspect private log".into());
        }
        if let Ok(jobs) = api(
            &client,
            &origin,
            "/jobs/overview?limit=100",
            &cookie,
            &csrf,
            "GET",
            None,
        )
        .await
            && ["running", "queued", "retry_wait"]
                .iter()
                .all(|kind| jobs[*kind].as_array().is_none_or(Vec::is_empty))
        {
            break;
        }
        if ready.elapsed() > Duration::from_secs(120) {
            return Err("server bootstrap did not drain".into());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    api(
        &client,
        &origin,
        "/memory/extraction",
        &cookie,
        &csrf,
        "PUT",
        Some(json!({"enabled":true,"request_timeout_seconds":300})),
    )
    .await?;
    let job = api(
        &client,
        &origin,
        "/memory/extraction/run",
        &cookie,
        &csrf,
        "POST",
        Some(json!({"include_evaluated":false})),
    )
    .await?;
    api(
        &client,
        &origin,
        "/index/embeddings/rebuild",
        &cookie,
        &csrf,
        "POST",
        Some(json!({})),
    )
    .await?;
    println!(
        "{}",
        json!({"event":"real_generation_started","run_root":root,"model":model.external_model_id,"embedding_model":embedding.external_model_id,"sources":manifest["source_count"],"job_id":job["id"]})
    );
    let job_id = job["id"].as_str().ok_or("job ID missing")?;
    loop {
        let status = api(
            &client,
            &origin,
            &format!("/jobs/{job_id}"),
            &cookie,
            &csrf,
            "GET",
            None,
        )
        .await?;
        save(&root.join("extraction-job.json"), &status)?;
        let progress = &status["progress"];
        println!(
            "{}",
            json!({"event":"real_generation_progress","status":status["status"],"completed_sources":progress["completed"],"total_sources":progress["total"],"units":progress["items_published"],"output_failures":progress["generated_output_failures"]})
        );
        if matches!(
            status["status"].as_str(),
            Some("completed" | "failed" | "cancelled")
        ) {
            break;
        }
        if server.0.try_wait()?.is_some() {
            return Err("server exited during generation".into());
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    api(
        &client,
        &origin,
        "/memory/embeddings/rebuild",
        &cookie,
        &csrf,
        "POST",
        Some(json!({})),
    )
    .await?;
    let deadline = Instant::now();
    loop {
        let generation = api(
            &client,
            &origin,
            "/memory/generation",
            &cookie,
            &csrf,
            "GET",
            None,
        )
        .await?;
        let vectors = api(
            &client,
            &origin,
            "/memory/embeddings",
            &cookie,
            &csrf,
            "GET",
            None,
        )
        .await?;
        let note_vectors = api(
            &client,
            &origin,
            "/index/status",
            &cookie,
            &csrf,
            "GET",
            None,
        )
        .await?;
        save(&root.join("generation-status.json"), &generation)?;
        save(
            &root.join("vector-status.json"),
            &json!({"memory":vectors,"note":note_vectors["note_semantic"]}),
        )?;
        if generation["runtime"]["generation"] == generation["runtime"]["overview_generation"]
            && vectors["current"] == vectors["eligible"]
            && note_vectors["note_semantic"]["indexed_chunks"]
                == note_vectors["note_semantic"]["source_chunks"]
        {
            break;
        }
        println!(
            "{}",
            json!({"event":"real_projection_progress","units":generation["counts"]["total"],"memory_vectors":vectors["current"],"eligible_memory_vectors":vectors["eligible"],"overview_active":!generation["overview_job"].is_null()})
        );
        if deadline.elapsed() > Duration::from_secs(900) {
            break;
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    let mut records = Vec::new();
    let mut offset = 0;
    loop {
        let page = api(
            &client,
            &origin,
            &format!("/memories?limit=200&offset={offset}"),
            &cookie,
            &csrf,
            "GET",
            None,
        )
        .await?;
        let current = page["memories"].as_array().ok_or("memory page missing")?;
        records.extend(current.clone());
        if current.len() < 200 {
            break;
        }
        offset += 200;
    }
    save(&root.join("units.json"), &json!({"memories":records}))?;
    save(
        &root.join("overview.json"),
        &api(
            &client,
            &origin,
            "/memory/overview?limit=40&max_tokens=32000",
            &cookie,
            &csrf,
            "GET",
            None,
        )
        .await?,
    )?;
    println!(
        "{}",
        json!({"event":"real_generation_available_for_review","run_root":root,"server_pid":server.0.id(),"admin_origin":origin,"units":records.len()})
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
    use mcp_vault_auth::MasterKeyRing;
    use mcp_vault_domain::VaultSlug;
    use mcp_vault_providers::{
        ModelCapabilities, ModelInput, ProviderInput, ProviderKind, ProviderSettings,
    };
    use mcp_vault_state::StateStore;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::Mutex;

    #[derive(Clone)]
    struct FakeProviderState {
        calls: Arc<AtomicUsize>,
        schemas: Arc<Mutex<Vec<String>>>,
    }

    async fn fake_selection(
        State(state): State<FakeProviderState>,
        Json(request): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        let Some(schema) = request
            .pointer("/response_format/json_schema/name")
            .and_then(Value::as_str)
        else {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"schema missing"})),
            );
        };
        state.calls.fetch_add(1, Ordering::SeqCst);
        state.schemas.lock().await.push(schema.to_owned());
        if schema != "memory_unit_selection" {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"unexpected schema"})),
            );
        }
        let input: Value = match request["messages"][1]["content"]
            .as_str()
            .and_then(|content| serde_json::from_str(content).ok())
        {
            Some(input) => input,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":"selection input missing"})),
                );
            }
        };
        let unit_id = input["units"][0]["unit_id"].clone();
        (
            StatusCode::OK,
            Json(json!({
                "choices": [{"message": {"content": json!({"selections": [{"unit_id": unit_id, "kind": "procedure", "retrieval_hint": "local fake selection"}]}).to_string()}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1}
            })),
        )
    }

    #[test]
    fn selection_manifest_guard_requires_matching_unique_hashed_sources() {
        let manifest = json!({
            "source_count": 1,
            "sources": [{"path": "notes/one.md", "sha256": "a".repeat(64)}]
        });
        assert!(validate_selection_manifest(&manifest).is_ok());

        let duplicate = json!({
            "source_count": 2,
            "sources": [
                {"path": "notes/one.md", "sha256": "a".repeat(64)},
                {"path": "notes/one.md", "sha256": "b".repeat(64)}
            ]
        });
        assert!(validate_selection_manifest(&duplicate).is_err());

        let mismatched_count = json!({
            "source_count": 2,
            "sources": [{"path": "notes/one.md", "sha256": "a".repeat(64)}]
        });
        assert!(validate_selection_manifest(&mismatched_count).is_err());

        let non_markdown = json!({
            "source_count": 1,
            "sources": [{"path": "notes/image.png", "sha256": "a".repeat(64)}]
        });
        assert!(validate_selection_manifest(&non_markdown).is_err());
    }

    #[tokio::test]
    async fn selection_only_rejects_wrong_arity_before_touching_inputs() {
        let error =
            run_authorized_selection_only(&["--run-authorized-real-selection-only".to_owned()])
                .await
                .unwrap_err()
                .to_string();
        assert!(error.contains("selection-only"));
    }

    #[tokio::test]
    async fn selection_only_uses_only_selection_schema_and_writes_complete_units() {
        let fixture = tempfile::tempdir().unwrap();
        let source_db = fixture.path().join("source.sqlite3");
        let source_root = fixture.path().join("source-vault");
        let source_database = format!("sqlite://{}", source_db.display());
        let source_state = StateStore::connect_and_migrate(&source_database)
            .await
            .unwrap();
        let source_context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("default").unwrap(),
            source_root,
            Revision::ZERO,
        )
        .unwrap();
        source_state
            .vaults()
            .insert(&source_context, "source", VaultStatus::Active)
            .await
            .unwrap();

        let fake_state = FakeProviderState {
            calls: Arc::new(AtomicUsize::new(0)),
            schemas: Arc::new(Mutex::new(Vec::new())),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let fake_state_for_server = fake_state.clone();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/v1/chat/completions", post(fake_selection))
                    .with_state(fake_state_for_server),
            )
            .await
            .unwrap();
        });
        let source_auth = AuthService::new(
            source_state.auth(),
            MasterKeyRing::from_bytes(1, &[23u8; 32]).unwrap(),
        );
        let source_providers = ProviderService::new(source_state.clone(), source_auth);
        let provider = source_providers
            .create_provider(ProviderInput {
                name: "local selection fake".into(),
                kind: ProviderKind::OpenAiCompatible,
                base_url: url::Url::parse(&format!("http://{address}/v1/")).unwrap(),
                settings: ProviderSettings::default(),
                enabled: true,
                secret: None,
            })
            .await
            .unwrap();
        let model = source_providers
            .register_model(ModelInput {
                provider_id: provider.id,
                external_model_id: "selection-fake".into(),
                capabilities: ModelCapabilities {
                    structured_output: true,
                    max_output_tokens: Some(2048),
                    ..Default::default()
                },
                settings: Default::default(),
                enabled: true,
            })
            .await
            .unwrap();
        source_providers
            .bind_model(
                Some(&source_context),
                "memory_extraction",
                model.id,
                json!({}),
                None,
            )
            .await
            .unwrap();
        let master_key = fixture.path().join("source-master-key");
        std::fs::write(&master_key, [23u8; 32]).unwrap();
        source_state.close().await;

        let notes = fixture.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let note_body = "# Accepted procedure\nCheck the device, then move the model.\n";
        std::fs::write(notes.join("one.md"), note_body).unwrap();
        let manifest_path = fixture.path().join("manifest.json");
        std::fs::write(
            &manifest_path,
            serde_json::to_vec(&json!({
                "schema": 1,
                "source_count": 1,
                "sources": [{"path":"one.md", "sha256":hash(note_body.as_bytes())}]
            }))
            .unwrap(),
        )
        .unwrap();
        let run_root = fixture.path().join("selection-run");
        let args = vec![
            "--run-authorized-real-selection-only".to_owned(),
            source_db.display().to_string(),
            master_key.display().to_string(),
            manifest_path.display().to_string(),
            run_root.display().to_string(),
        ];
        let selection_result =
            run_authorized_selection_only_with_mode(&args, ProviderMode::LocalOnly).await;
        assert!(
            selection_result.is_ok(),
            "selection-only failed: {:?}; calls={}; schemas={:?}",
            selection_result.err(),
            fake_state.calls.load(Ordering::SeqCst),
            fake_state.schemas.lock().await
        );

        let run: Value =
            serde_json::from_slice(&std::fs::read(run_root.join("run.json")).unwrap()).unwrap();
        assert_eq!(run["mode"], "real_selection_only_real_llm_original_sources");
        assert_eq!(run["workers_started"], false);
        assert_eq!(run["http_server_started"], false);
        assert_eq!(
            run["actual_provider_requests"],
            "not_available_from_memory_service"
        );
        let units: Value =
            serde_json::from_slice(&std::fs::read(run_root.join("units.json")).unwrap()).unwrap();
        assert_eq!(units["memories"].as_array().unwrap().len(), 1);
        assert_eq!(units["memories"][0]["content"], note_body);
        assert_eq!(fake_state.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            fake_state.schemas.lock().await.as_slice(),
            ["memory_unit_selection".to_owned()]
        );
        assert!(!run_root.join("server.log").exists());

        let target_state = StateStore::connect(&format!(
            "sqlite://{}/state/mcp-vault.sqlite3",
            run_root.display()
        ))
        .await
        .unwrap();
        let target_context = target_state
            .vaults()
            .find_by_slug(&VaultSlug::new("default").unwrap())
            .await
            .unwrap()
            .unwrap()
            .context()
            .unwrap();
        assert!(
            target_state
                .providers()
                .resolve_binding(&target_context, "embedding_memory")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            target_state
                .providers()
                .resolve_binding(&target_context, "memory_overview")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            target_state
                .jobs()
                .list(&target_context, None, None, 200, 0)
                .await
                .unwrap()
                .is_empty()
        );
        target_state.close().await;

        let duplicate = run_authorized_selection_only(&args)
            .await
            .unwrap_err()
            .to_string();
        assert!(duplicate.contains("must not already exist"));
    }
}
