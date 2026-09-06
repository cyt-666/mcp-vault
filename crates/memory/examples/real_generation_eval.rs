//! Explicitly authorized, bounded real generation evaluation on synthetic notes.
//! Source installation is read-only; all mutations use a disposable Vault Core.
use async_trait::async_trait;
use mcp_vault_auth::{AuthService, MasterKeyRing, load_or_create_master_key};
use mcp_vault_core::VaultCore;
use mcp_vault_domain::{Actor, Revision, SourcePlane, VaultContext, VaultId, VaultPath, VaultSlug};
use mcp_vault_memory::{ExtractionPolicy, MemoryService};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderError, ProviderInput, ProviderKind,
    ProviderService, ProviderSettings, RequestBudget,
};
use mcp_vault_state::{StateStore, VaultStatus};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::io::AsyncWriteExt;

struct Budget {
    spent: AtomicUsize,
    ledger: PathBuf,
}

#[async_trait]
impl RequestBudget for Budget {
    async fn reserve(&self, body_bytes: usize) -> Result<(), ProviderError> {
        let prior = self
            .spent
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < 15).then_some(n + 1)
            })
            .map_err(|_| ProviderError::InvalidConfiguration("evaluation_request_limit"))?;
        // Accounting reaches disk before dispatch. A failed ledger write still
        // consumes this attempt; the command never refunds or resumes a run.
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&self.ledger)
            .await
            .map_err(|_| ProviderError::InvalidConfiguration("evaluation_ledger_unavailable"))?;
        file.write_all(
            format!("{}\n", json!({"attempt":prior+1,"body_bytes":body_bytes})).as_bytes(),
        )
        .await
        .map_err(|_| ProviderError::InvalidConfiguration("evaluation_ledger_unavailable"))?;
        file.sync_all()
            .await
            .map_err(|_| ProviderError::InvalidConfiguration("evaluation_ledger_unavailable"))?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 6
        || args[0] != "--source-data"
        || args[2] != "--output"
        || args[4] != "--authorized-requests"
        || args[5] != "15"
    {
        return Err("usage: real_generation_eval --source-data DIR --output NEW_DIR --authorized-requests 15".into());
    }
    let source_root = PathBuf::from(&args[1]);
    let output = PathBuf::from(&args[3]);
    let source = StateStore::connect_read_only(&format!(
        "sqlite://{}",
        source_root.join("state/mcp-vault.sqlite3").display()
    ))
    .await?;
    let source_context = source
        .vaults()
        .find_by_slug(&VaultSlug::new("default")?)
        .await?
        .ok_or("source Vault missing")?
        .context()?;
    let binding = source
        .providers()
        .resolve_binding(&source_context, "memory_extraction")
        .await?
        .ok_or("bind memory_extraction in the local Admin before running")?;
    let model = source
        .providers()
        .get_model(binding.model_id)
        .await?
        .ok_or("model missing")?;
    let provider = source
        .providers()
        .get_provider(model.provider_id)
        .await?
        .ok_or("provider missing")?;
    if !provider.enabled || !model.enabled {
        return Err("selected model/provider disabled".into());
    }
    let keys = MasterKeyRing::load_file(&source_root.join("secrets/master-key")).await?;
    let key_check = source
        .auth()
        .get_installation_key_check(keys.current_version())
        .await?
        .ok_or("installation key check missing")?;
    if !keys.matches_installation_key_check(&key_check) {
        return Err("installation key mismatch".into());
    }
    let source_auth = AuthService::new(source.auth(), keys);
    let source_service = ProviderService::new(source.clone(), source_auth.clone());
    let mode = source_service.provider_mode(&source_context).await?;
    if mode == mcp_vault_providers::ProviderMode::Disabled {
        return Err("Provider mode disabled".into());
    }
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
    // Refuse reuse of an output directory, so a repeated command cannot silently
    // spend another authorized round or overwrite evidence of a partial run.
    tokio::fs::create_dir(&output).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o700)).await?;
    }
    let ledger = output.join("requests.jsonl");
    tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&ledger)
        .await?;
    let budget = Arc::new(Budget {
        spent: AtomicUsize::new(0),
        ledger,
    });
    let temporary = tempfile::tempdir()?;
    let state = StateStore::connect_and_migrate("sqlite::memory:").await?;
    let eval_keys = load_or_create_master_key(&temporary.path().join("master-key")).await?;
    let auth = AuthService::new(state.auth(), eval_keys);
    let providers =
        ProviderService::new(state.clone(), auth).with_generation_budget(budget.clone());
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("real-generation-eval")?,
        temporary.path().join("vault"),
        Revision::ZERO,
    )?;
    state
        .vaults()
        .insert(
            &context,
            "Synthetic real generation evaluation",
            VaultStatus::Active,
        )
        .await?;
    providers.set_provider_mode(&context, mode, None).await?;
    let mut settings = ProviderSettings::from_json(&provider.settings)?;
    settings.max_retries = 0;
    let eval_provider = providers
        .create_provider(ProviderInput {
            name: "authorized synthetic evaluation".into(),
            kind: ProviderKind::try_from(provider.provider_type.as_str())?,
            base_url: provider.base_url.parse()?,
            settings,
            enabled: true,
            secret,
        })
        .await?;
    let eval_model = providers
        .register_model(ModelInput {
            provider_id: eval_provider.id,
            external_model_id: model.external_model_id.clone(),
            capabilities: ModelCapabilities::from_json(&model.capabilities)?,
            settings: ModelSettings::from_json(&model.settings)?,
            enabled: true,
        })
        .await?;
    providers
        .bind_model(
            Some(&context),
            "memory_extraction",
            eval_model.id,
            binding.settings,
            None,
        )
        .await?;
    let service = MemoryService::with_provider_service(state.clone(), providers);
    service
        .set_extraction_policy(
            &context,
            ExtractionPolicy {
                enabled: true,
                request_timeout_seconds: 180,
                ..Default::default()
            },
            None,
            None,
        )
        .await?;
    let core = VaultCore::new(
        state.clone(),
        temporary.path().join("history"),
        Default::default(),
        Default::default(),
        Default::default(),
    );
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/memory-quality/generation-coverage.json"
    ))?;
    let cases = corpus["cases"].as_array().ok_or("invalid cases")?;
    if cases.len() != 15 {
        return Err("authorized corpus must contain exactly 15 cases".into());
    }
    let mut results = Vec::new();
    for case in cases {
        let id = case["id"].as_str().ok_or("case id missing")?;
        let path = VaultPath::parse(&format!("{id}.md"))?;
        core.create_bytes(
            &context,
            &path,
            case["source"].as_str().ok_or("source missing")?.as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await?;
        let before = budget.spent.load(Ordering::SeqCst);
        let extraction = service.extract_note(&context, &core, &path).await;
        let mut contents = Vec::new();
        let mut unchanged_skipped = false;
        let mut error = None;
        match extraction {
            Ok(result) => {
                let file = core.read(&context, &path).await?;
                if let Some(set) = state
                    .current_memory()
                    .get_note_set_by_source(&context, file.file.id)
                    .await?
                {
                    for item in state
                        .current_memory()
                        .list_note_set_items(&context, set.id)
                        .await?
                    {
                        contents.push(json!({"content":item.memory.content,"kind":item.memory.kind,"tags":item.memory.tags}));
                    }
                }
                let used = budget.spent.load(Ordering::SeqCst);
                let repeated = service.extract_note(&context, &core, &path).await?;
                unchanged_skipped =
                    repeated.already_evaluated && budget.spent.load(Ordering::SeqCst) == used;
                if !result.source_admitted || !unchanged_skipped {
                    error = Some("evaluation_source_or_idempotency_failure");
                }
            }
            Err(failure) => error = Some(failure.code()),
        }
        let detail = json!({"id":id,"language":case["language"],"source":case["source"],
            "required_facts":case["required_facts"],"forbidden_claims":case["forbidden_claims"],
            "memories":contents,"error":error,"unchanged_source_skipped":unchanged_skipped,
            "actual_requests":budget.spent.load(Ordering::SeqCst)-before,"quality_review":"pending"});
        tokio::fs::write(
            output.join(format!("{id}.json")),
            serde_json::to_vec_pretty(&detail)?,
        )
        .await?;
        results.push(detail);
        tokio::fs::write(output.join("report.json"),serde_json::to_vec_pretty(&json!({
            "scope":"synthetic_non_private_real_generation","implementation_version":env!("CARGO_PKG_VERSION"),
            "pipeline":mcp_vault_memory::EXTRACTION_PIPELINE_VERSION,"model":model.external_model_id,
            "authorized_requests":15,"actual_requests":budget.spent.load(Ordering::SeqCst),
            "source_installation_modified":false,"quality_review":"pending","cases":results
        }))?).await?;
        println!(
            "{id}: requests={}, items={}, error={}",
            budget.spent.load(Ordering::SeqCst) - before,
            contents.len(),
            error.unwrap_or("none")
        );
        if budget.spent.load(Ordering::SeqCst) >= 15 {
            break;
        }
    }
    Ok(())
}
