//! Run exactly one Phase 1 extraction against an isolated data-directory copy.
//! `scripts/debug/phase2-replay.sh` can prepare the required sentinel and copied
//! Vault without issuing a Phase 2 request.

use std::{error::Error, path::PathBuf};

use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_core::{VaultCore, VaultCoreRuntime};
use mcp_vault_domain::{VaultPath, VaultPathPolicy};
use mcp_vault_memory::{MemoryService, NoteExtractionOptions};
use mcp_vault_providers::ProviderService;
use mcp_vault_state::{StateStore, VaultStatus};
use mcp_vault_storage_fs::StorageOptions;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let data_dir = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("usage: memory_phase1_replay <isolated-data-directory> <vault-relative-path>")?
        .canonicalize()?;
    let source_path = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or("usage: memory_phase1_replay <isolated-data-directory> <vault-relative-path>")?;
    if arguments.next().is_some() {
        return Err("memory_phase1_replay received unexpected arguments".into());
    }
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
    let source_path = VaultPath::parse(&source_path)?;

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
            "event": "memory_phase1_replay_started",
            "vault_id": context.id(),
        })
    );
    match memory
        .extract_note_with_options(
            &context,
            &core,
            &source_path,
            NoteExtractionOptions {
                include_evaluated: true,
            },
        )
        .await
    {
        Ok(report) => {
            println!(
                "{}",
                json!({
                    "event": "memory_phase1_replay_completed",
                    "source_admitted": report.source_admitted,
                    "raw_memory_staged": report.raw_memory_staged,
                    "no_output": report.no_output,
                    "already_evaluated": report.already_evaluated,
                })
            );
            Ok(())
        }
        Err(error) => {
            println!(
                "{}",
                json!({
                    "event": "memory_phase1_replay_failed",
                    "error_code": error.code(),
                    "retryable": error.retryable(),
                })
            );
            Err(error.into())
        }
    }
}
