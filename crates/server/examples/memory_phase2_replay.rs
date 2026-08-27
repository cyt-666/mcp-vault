//! Run exactly one Phase 2 consolidation against an isolated data-directory
//! copy. `scripts/debug/phase2-replay.sh` creates the required sentinel and
//! rewrites every Vault root into that copy before invoking this example.

use std::{error::Error, path::PathBuf};

use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_core::{VaultCore, VaultCoreRuntime};
use mcp_vault_domain::VaultPathPolicy;
use mcp_vault_memory::MemoryService;
use mcp_vault_providers::ProviderService;
use mcp_vault_state::{StateStore, VaultStatus};
use mcp_vault_storage_fs::StorageOptions;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let data_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: memory_phase2_replay <isolated-data-directory>")?
        .canonicalize()?;
    if !data_dir.join(".phase2-replay").is_file() {
        return Err("refusing to use a data directory without .phase2-replay".into());
    }

    let database_path = data_dir.join("state/mcp-vault.sqlite3");
    let database_url = format!("sqlite://{}", database_path.display());
    let state = StateStore::connect_and_migrate(&database_url).await?;
    let mut vaults = state
        .vaults()
        .list()
        .await?
        .into_iter()
        .filter(|vault| vault.status == VaultStatus::Active)
        .collect::<Vec<_>>();
    if vaults.len() != 1 {
        return Err(format!(
            "replay requires exactly one active Vault, found {}",
            vaults.len()
        )
        .into());
    }
    let vault = vaults.remove(0);
    let content_root = vault.content_root.canonicalize()?;
    if !content_root.starts_with(&data_dir) {
        return Err("replay Vault root escapes the isolated data directory".into());
    }
    let context = vault.context()?;
    let stage1 = state.memory().stage1_counts(&context).await?;
    if stage1.pending == 0 {
        return Err("replay has no pending Phase 1 input".into());
    }

    let keys = MasterKeyRing::load_file(&data_dir.join("secrets/master-key")).await?;
    let auth = AuthService::new(state.auth(), keys);
    let providers = ProviderService::new(state.clone(), auth);
    let memory = MemoryService::with_provider_service(state.clone(), providers);
    let core = VaultCore::new(
        state,
        data_dir.join("history"),
        VaultPathPolicy::new(vault.reserved_root, Default::default())?,
        StorageOptions::default(),
        VaultCoreRuntime::default(),
    );

    println!(
        "{}",
        json!({
            "event": "memory_phase2_replay_started",
            "vault_id": context.id(),
            "pending": stage1.pending,
            "ready": stage1.ready,
            "no_output": stage1.no_output,
            "withdrawn": stage1.withdrawn,
        })
    );
    match memory.consolidate(&context, &core).await {
        Ok(report) => {
            println!(
                "{}",
                json!({
                    "event": "memory_phase2_replay_completed",
                    "raw_inputs": report.raw_inputs,
                    "created": report.created,
                    "updated": report.updated,
                    "retired": report.retired,
                    "discarded": report.discarded,
                    "generation": report.generation,
                    "reused_proposal": report.reused_proposal,
                })
            );
            Ok(())
        }
        Err(error) => {
            println!(
                "{}",
                json!({
                    "event": "memory_phase2_replay_failed",
                    "error_code": error.code(),
                    "retryable": error.retryable(),
                })
            );
            Err(error.into())
        }
    }
}
