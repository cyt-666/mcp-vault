//! Explicit offline cutover command; it never starts listeners or Providers.
use crate::{AppConfig, ServerError, core_for_vault, resolve_runtime_path};
use mcp_vault_domain::VaultPath;
use mcp_vault_state::StateStore;
use serde_json::{Value, json};

/// Read-only status for a failed or interrupted offline cutover. This path
/// never migrates, acquires the maintenance gate, runs recovery, or touches
/// canonical files.
pub async fn inspect_memory_initialization(config: &AppConfig) -> Result<Value, ServerError> {
    let state = StateStore::connect_read_only(&config.database_url).await?;
    let mut vaults = Vec::new();
    for vault in state.vaults().list().await? {
        let context = vault.context()?;
        let initialization = state.memory_units().initialization(&context).await?;
        let journals = state
            .files()
            .summarize_incomplete(&context)
            .await?
            .into_iter()
            .map(|summary| {
                json!({
                    "operation": summary.operation.as_str(),
                    "state": summary.state.as_str(),
                    "count": summary.count,
                })
            })
            .collect::<Vec<_>>();
        vaults.push(json!({
            "vault_id": context.id(),
            "vault_slug": context.slug(),
            "phase": initialization.as_ref().map(|state| state.phase.as_str()).unwrap_or("missing"),
            "manifest": initialization.as_ref().map(|state| json!({
                "completed_files": state.manifest["completed_files"],
                "file_count": state.manifest["files"].as_array().map_or(0, Vec::len),
            })),
            "legacy_record_counts": state.memory_units().legacy_counts(&context).await?,
            "incomplete_journals": journals,
        }));
    }
    state.close().await;
    Ok(json!({
        "operation": "inspect_memory_initialization",
        "read_only": true,
        "migrated": false,
        "recovered": false,
        "provider_called": false,
        "vaults": vaults,
    }))
}

pub async fn initialize_memory(config: &AppConfig) -> Result<Value, ServerError> {
    let _database_lock = StateStore::acquire_process_lock(&config.database_url)?;
    let state = StateStore::connect_offline_exclusive(&config.database_url).await?;
    state.migrate().await?;
    let integrity = state.integrity_check().await?;
    if !integrity.integrity_ok || integrity.foreign_key_violations != 0 {
        return Err(ServerError::State(
            mcp_vault_state::StateError::IntegrityFailure,
        ));
    }
    let runtime = mcp_vault_core::VaultCoreRuntime::default();
    let permit = runtime.maintenance_recovery_permit();
    let history = resolve_runtime_path(&config.data_dir)?.join("history");
    let mut reports = Vec::new();
    for vault in state.vaults().list().await? {
        let context = vault.context()?;
        let core = core_for_vault(&state, &history, &vault, &runtime)?;
        let preview = mcp_vault_memory::preview_memory_initialization(&state, &context, &core)
            .await
            .map_err(|error| {
                let details = error.initialization_failure_details();
                eprintln!(
                    "{}",
                    json!({"operation":"initialize_memory_failed","phase":"preview","vault_id":context.id(),"code":error.diagnostic_code(),"stage":details.as_ref().map(|item| item.0).unwrap_or("unknown"),"path":details.as_ref().and_then(|item| item.1.map(VaultPath::as_str)),"completed_files":details.as_ref().map(|item| item.2).unwrap_or(0)})
                );
                ServerError::MemoryInitialization(error.diagnostic_code())
            })?;
        eprintln!(
            "{}",
            json!({"operation":"initialize_memory_preview","preview":preview})
        );
        let report = mcp_vault_memory::initialize_vault_memories(&state, &context, &core, &permit)
            .await
            .map_err(|error| {
                let details = error.initialization_failure_details();
                eprintln!(
                    "{}",
                    json!({"operation":"initialize_memory_failed","phase":"cleanup","vault_id":context.id(),"code":error.diagnostic_code(),"stage":details.as_ref().map(|item| item.0).unwrap_or("unknown"),"path":details.as_ref().and_then(|item| item.1.map(VaultPath::as_str)),"completed_files":details.as_ref().map(|item| item.2).unwrap_or(0)})
                );
                ServerError::MemoryInitialization(error.diagnostic_code())
            })?;
        reports.push(report);
    }
    let integrity = state.integrity_check().await?;
    if !integrity.integrity_ok || integrity.foreign_key_violations != 0 {
        return Err(ServerError::State(
            mcp_vault_state::StateError::IntegrityFailure,
        ));
    }
    state.close().await;
    Ok(
        json!({"operation":"initialize_memory","contract":"source_preserving_memory_units_v3","vaults":reports,"next_step":"start_service_verify_login_webdav_mcp_then_resume_generation"}),
    )
}
