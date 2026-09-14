//! Offline canonical cleanup orchestration. No predecessor format reader exists.
use crate::MemoryError;
use mcp_vault_core::VaultCoreRuntime;
use mcp_vault_core::{MaintenanceRecoveryPermit, VaultCore};
use mcp_vault_domain::{
    Actor, MaintenanceGate, MaintenanceLease, MaintenanceMode, VaultContext, VaultId, VaultPath,
    VaultPathPolicy,
};
use mcp_vault_state::{StateStore, UnitRepository, VaultRecord};
use mcp_vault_storage_fs::StorageOptions;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FileToRetire {
    path: VaultPath,
    content_hash: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Manifest {
    schema: u32,
    files: Vec<FileToRetire>,
    original_counts: BTreeMap<String, u64>,
    completed_files: usize,
}
#[derive(Clone, Debug, Serialize)]
pub struct MemoryInitializationReport {
    pub vault_id: VaultId,
    pub vault_slug: String,
    pub outcome: String,
    pub file_count: usize,
    pub original_counts: BTreeMap<String, u64>,
    pub generation_paused: bool,
}

#[derive(Clone)]
pub struct MemoryInitializationService {
    state: StateStore,
    maintenance: MaintenanceGate,
    core_runtime: VaultCoreRuntime,
    history_root: PathBuf,
    storage_options: StorageOptions,
    dispatch: Arc<Mutex<BTreeSet<String>>>,
    handles: Arc<Mutex<BTreeMap<String, tokio::task::JoinHandle<()>>>>,
}

#[derive(Clone, Debug, Serialize)]
pub enum InitializationStart {
    Ready(Value),
    Accepted(Value),
}

impl MemoryInitializationService {
    pub fn new(
        state: StateStore,
        maintenance: MaintenanceGate,
        core_runtime: VaultCoreRuntime,
        history_root: PathBuf,
        storage_options: StorageOptions,
    ) -> Self {
        Self {
            state,
            maintenance,
            core_runtime,
            history_root,
            storage_options,
            dispatch: Arc::new(Mutex::new(BTreeSet::new())),
            handles: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn core_for_vault(&self, vault: &VaultRecord) -> Result<VaultCore, MemoryError> {
        let policy = VaultPathPolicy::new(vault.reserved_root.clone(), Default::default())
            .map_err(|_| MemoryError::InvalidInput("Vault path policy is invalid"))?;
        Ok(VaultCore::new(
            self.state.clone(),
            self.history_root.clone(),
            policy,
            self.storage_options,
            self.core_runtime.clone(),
        ))
    }

    pub async fn status(&self, vault: &VaultRecord) -> Result<Value, MemoryError> {
        let context = vault.context()?;
        let core = self.core_for_vault(vault)?;
        let initialization = self.state.memory_units().initialization(&context).await?;
        let task = self
            .state
            .memory_units()
            .initialization_task(&context)
            .await?;
        let journal_summary = self
            .state
            .files()
            .summarize_incomplete(&context)
            .await?
            .into_iter()
            .map(|item| {
                json!({
                    "operation": item.operation.as_str(),
                    "state": item.state.as_str(),
                    "count": item.count,
                })
            })
            .collect::<Vec<_>>();
        let preview =
            match preview_memory_initialization_for_admin(&self.state, &context, &core).await {
                Ok(preview) => preview,
                Err(error) => initialization_error_json(&error),
            };
        Ok(
            json!({"vault_status": vault.status.as_str(), "initialization":initialization,"preview":preview,"task":task,"journal_summary":journal_summary,"maintenance":self.maintenance.mode().as_str(),"read_only":true}),
        )
    }

    pub async fn start(
        &self,
        vault: &VaultRecord,
        resume_failed: bool,
    ) -> Result<InitializationStart, MemoryError> {
        let context = vault.context()?;
        let status = self.status(vault).await?;
        let phase = status
            .get("initialization")
            .and_then(|value| value.get("phase"))
            .and_then(Value::as_str)
            .or_else(|| {
                status
                    .get("preview")
                    .and_then(|value| value.get("phase"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("ready");
        if phase == "ready" {
            return Ok(InitializationStart::Ready(status));
        }
        let existing_task_state = status
            .get("task")
            .and_then(|value| value.get("state"))
            .and_then(Value::as_str);
        if self.maintenance.mode() == MaintenanceMode::Offline
            && !(resume_failed && existing_task_state == Some("failed"))
        {
            return Err(MemoryError::Configuration("maintenance_recovery_required"));
        }
        let key = context.id().to_string();
        {
            let mut dispatch = self.dispatch.lock().await;
            if dispatch.contains(&key) {
                return Ok(InitializationStart::Accepted(status));
            }
            dispatch.insert(key.clone());
        }
        let total_files = status
            .get("preview")
            .and_then(|value| value.get("files"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len) as u64;
        let existing_task = self
            .state
            .memory_units()
            .initialization_task(&context)
            .await?;
        let resume_existing = resume_failed
            && existing_task
                .as_ref()
                .is_some_and(|task| matches!(task.state.as_str(), "queued" | "running"));
        let task_id = if resume_existing {
            existing_task
                .as_ref()
                .expect("resume task exists")
                .task_id
                .clone()
        } else {
            mcp_vault_domain::OperationId::new().to_string()
        };
        let task = if resume_existing {
            existing_task.expect("resume task exists")
        } else {
            match self
                .state
                .memory_units()
                .queue_initialization_task(
                    &context,
                    &task_id,
                    total_files,
                    resume_failed,
                    self.maintenance.mode().as_str(),
                )
                .await
            {
                Ok(task) => task,
                Err(error) => {
                    self.dispatch.lock().await.remove(&key);
                    return Err(error.into());
                }
            }
        };
        let worker_vault = vault.clone();
        let worker = self.clone();
        let handle = tokio::spawn(async move {
            worker.run(worker_vault, task_id).await;
            worker.dispatch.lock().await.remove(&key);
        });
        self.handles
            .lock()
            .await
            .insert(context.id().to_string(), handle);
        Ok(InitializationStart::Accepted(json!({"task":task})))
    }

    /// Abort in-process initialization tasks during graceful shutdown. The
    /// durable task row remains queued/running for explicit Admin resume.
    pub async fn shutdown(&self) {
        let mut handles = self.handles.lock().await;
        let tasks = std::mem::take(&mut *handles);
        for (_, handle) in tasks {
            handle.abort();
            let _ = handle.await;
        }
        self.dispatch.lock().await.clear();
    }

    async fn run(&self, vault: VaultRecord, task_id: String) {
        let context = match vault.context() {
            Ok(context) => context,
            Err(_) => return,
        };
        let Some(mut lease) = self.maintenance.try_begin_offline() else {
            let _ = self
                .state
                .memory_units()
                .fail_initialization_task(&context, &task_id, "maintenance_busy", 0)
                .await;
            return;
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while !lease.is_drained() {
            if tokio::time::Instant::now() >= deadline {
                let _ = self
                    .state
                    .memory_units()
                    .fail_initialization_task(&context, &task_id, "maintenance_drain_timeout", 0)
                    .await;
                lease.restore();
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let task_meta = self
            .state
            .memory_units()
            .initialization_task(&context)
            .await
            .ok()
            .flatten();
        if self
            .state
            .memory_units()
            .start_initialization_task(&context, &task_id)
            .await
            .is_err()
        {
            let _ = self
                .state
                .memory_units()
                .fail_initialization_task(
                    &context,
                    &task_id,
                    "initialization_task_claim_conflict",
                    0,
                )
                .await;
            lease.restore();
            return;
        }
        let core = match self.core_for_vault(&vault) {
            Ok(core) => core,
            Err(_) => {
                let _ = self
                    .state
                    .memory_units()
                    .fail_initialization_task(&context, &task_id, "core_unavailable", 0)
                    .await;
                return;
            }
        };
        let permit = self
            .core_runtime
            .maintenance_recovery_permit_with_lease(&lease);
        match initialize_vault_memories_with_lease(&self.state, &context, &core, &permit, &lease)
            .await
        {
            Ok(report) => {
                if self
                    .state
                    .memory_units()
                    .finish_initialization_task(&context, &task_id, report.file_count as u64)
                    .await
                    .is_err()
                {
                    // Canonical cleanup is already ready. Retry only the
                    // terminal metadata update; never run the file loop again.
                    if self
                        .state
                        .memory_units()
                        .reconcile_ready_initialization_task(&context, report.file_count as u64)
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                let previous = task_meta
                    .as_ref()
                    .map(|task| task.maintenance_previous_mode.as_str())
                    .unwrap_or("normal");
                lease.restore_to(match previous {
                    "read_only" => MaintenanceMode::ReadOnly,
                    "offline" => MaintenanceMode::Offline,
                    _ => MaintenanceMode::Normal,
                });
            }
            Err(error) => {
                let details = error.initialization_failure_details();
                let completed = details.map_or(0, |item| item.2) as u64;
                let _ = self
                    .state
                    .memory_units()
                    .fail_initialization_task_with_diagnostic(
                        &context,
                        &task_id,
                        error.diagnostic_code(),
                        details.map(|item| item.0),
                        details.and_then(|item| item.1.map(VaultPath::as_str)),
                        details.map(|item| item.3),
                        completed,
                    )
                    .await;
            }
        }
    }
}

/// Allow-list only the registered predecessor memory namespace. New units live
/// beside it in memory-v3; arbitrary files elsewhere are never cleanup inputs.
fn legacy_path(core: &VaultCore, path: &VaultPath) -> bool {
    let prefix = format!("{}/", core.managed_root());
    let Some(relative) = path.as_str().strip_prefix(&prefix) else {
        return false;
    };
    relative.starts_with("memory/")
}

fn initialization_failure(
    stage: &'static str,
    path: Option<VaultPath>,
    completed_files: usize,
    source: MemoryError,
) -> MemoryError {
    MemoryError::InitializationFailure {
        stage,
        path,
        completed_files,
        source: Box::new(source),
    }
}

fn initialization_error_json(error: &MemoryError) -> Value {
    let details = error.initialization_failure_details();
    json!({
        "available": false,
        "error_code": error.diagnostic_code(),
        "diagnostic": {
            "stage": details.map(|item| item.0),
            "path": details.and_then(|item| item.1.map(VaultPath::as_str)),
            "completed_files": details.map_or(0, |item| item.2),
            "source_code": details.map(|item| item.3),
        },
    })
}

/// Content-free inventory for an exclusively locked offline command. No
/// canonical mutation or legacy format parsing occurs during this preview.
pub async fn preview_memory_initialization(
    state: &StateStore,
    context: &VaultContext,
    core: &VaultCore,
) -> Result<serde_json::Value, MemoryError> {
    if !state.is_offline_exclusive() {
        return Err(MemoryError::InvalidInput(
            "memory initialization requires exclusive offline access",
        ));
    }
    let previous = state.memory_units().initialization(context).await?;
    if previous.as_ref().is_none_or(|row| row.phase == "ready") {
        return Ok(json!({"vault_id":context.id(),"phase":"ready","files":[],"record_counts":{}}));
    }
    if let Some(previous) = previous.as_ref().filter(|row| row.phase == "clearing") {
        let manifest: Manifest = serde_json::from_value(previous.manifest.clone())
            .map_err(|_| MemoryError::InvalidInput("memory initialization manifest is invalid"))?;
        return Ok(
            json!({"vault_id":context.id(),"phase":"clearing","files":manifest.files.iter().map(|file|&file.path).collect::<Vec<_>>(),"record_counts":manifest.original_counts,"completed_files":manifest.completed_files}),
        );
    }
    let mut paths = BTreeSet::new();
    for entry in core
        .list_managed_files(context)
        .await
        .map_err(|source| initialization_failure("preview_scan", None, 0, source.into()))?
    {
        if entry.kind == mcp_vault_domain::FilesystemEntryKind::RegularFile
            && let Some(path) = entry.path.filter(|path| legacy_path(core, path))
        {
            paths.insert(path);
        }
    }
    for file in state.files().list_active_entries(context).await? {
        if file.entry_type == mcp_vault_state::EntryType::File && legacy_path(core, &file.path) {
            paths.insert(file.path);
        }
    }
    Ok(
        json!({"vault_id":context.id(),"phase":"required","files":paths,"record_counts":state.memory_units().legacy_counts(context).await?}),
    )
}

/// Read-only Admin preview. It does not require the offline-exclusive store;
/// the caller still supplies a Vault-scoped Core and only inventory is read.
pub async fn preview_memory_initialization_for_admin(
    state: &StateStore,
    context: &VaultContext,
    core: &VaultCore,
) -> Result<serde_json::Value, MemoryError> {
    let previous = state.memory_units().initialization(context).await?;
    if previous.as_ref().is_none_or(|row| row.phase == "ready") {
        return Ok(json!({"vault_id":context.id(),"phase":"ready","files":[],"record_counts":{}}));
    }
    if let Some(previous) = previous.as_ref().filter(|row| row.phase == "clearing") {
        let manifest: Manifest = serde_json::from_value(previous.manifest.clone())
            .map_err(|_| MemoryError::InvalidInput("memory initialization manifest is invalid"))?;
        return Ok(
            json!({"vault_id":context.id(),"phase":"clearing","files":manifest.files.iter().map(|file|&file.path).collect::<Vec<_>>(),"record_counts":manifest.original_counts,"completed_files":manifest.completed_files}),
        );
    }
    let mut paths = BTreeSet::new();
    for entry in core
        .list_managed_files(context)
        .await
        .map_err(|source| initialization_failure("preview_scan", None, 0, source.into()))?
    {
        if entry.kind == mcp_vault_domain::FilesystemEntryKind::RegularFile
            && let Some(path) = entry.path.filter(|path| legacy_path(core, path))
        {
            paths.insert(path);
        }
    }
    for file in state.files().list_active_entries(context).await? {
        if file.entry_type == mcp_vault_state::EntryType::File && legacy_path(core, &file.path) {
            paths.insert(file.path);
        }
    }
    Ok(
        json!({"vault_id":context.id(),"phase":"required","files":paths,"record_counts":state.memory_units().legacy_counts(context).await?}),
    )
}

/// Caller must own the database process lock and an EXCLUSIVE offline store.
/// Returns only names/counts; no old body, provider secret, or credential is read
/// into a report. Re-running after ready skips recovery, scans and all mutations.
pub async fn initialize_vault_memories(
    state: &StateStore,
    context: &VaultContext,
    core: &VaultCore,
    permit: &MaintenanceRecoveryPermit,
) -> Result<MemoryInitializationReport, MemoryError> {
    let units = state.memory_units();
    initialize_vault_memories_with_repository(state, context, core, permit, &units, false).await
}

/// Online Admin worker entry point. The caller must hold the shared Offline
/// lease; only the memory initialization repository receives the controlled
/// write permit while the rest of the StateStore remains online.
pub async fn initialize_vault_memories_with_lease(
    state: &StateStore,
    context: &VaultContext,
    core: &VaultCore,
    permit: &MaintenanceRecoveryPermit,
    lease: &MaintenanceLease,
) -> Result<MemoryInitializationReport, MemoryError> {
    let units = state.memory_units_for_initialization(lease);
    initialize_vault_memories_with_repository(state, context, core, permit, &units, true).await
}

async fn initialize_vault_memories_with_repository(
    state: &StateStore,
    context: &VaultContext,
    core: &VaultCore,
    permit: &MaintenanceRecoveryPermit,
    units: &UnitRepository,
    controlled_online: bool,
) -> Result<MemoryInitializationReport, MemoryError> {
    if !state.is_offline_exclusive() && !controlled_online {
        return Err(MemoryError::InvalidInput(
            "memory initialization requires exclusive offline access",
        ));
    }
    if controlled_online {
        core.validate_maintenance_permit(permit)?;
    }
    let previous = units.initialization(context).await?;
    if previous.as_ref().is_none_or(|row| row.phase == "ready") {
        return Ok(MemoryInitializationReport {
            vault_id: context.id(),
            vault_slug: context.slug().to_string(),
            outcome: "already_initialized".into(),
            file_count: 0,
            original_counts: BTreeMap::new(),
            generation_paused: state.memory_units().runtime(context).await?.paused,
        });
    }
    let saved_completed_files = previous
        .as_ref()
        .and_then(|row| row.manifest.get("completed_files"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0);
    let mut manifest = if let Some(previous) =
        previous.as_ref().filter(|row| row.phase == "clearing")
    {
        serde_json::from_value::<Manifest>(previous.manifest.clone())
            .map_err(|_| MemoryError::InvalidInput("memory initialization manifest is invalid"))?
    } else {
        let mut paths = BTreeSet::new();
        let entries = if controlled_online {
            core.list_managed_files_during_maintenance(context, permit)
                .await
        } else {
            core.list_managed_files(context).await
        }
        .map_err(|source| initialization_failure("build_manifest", None, 0, source.into()))?;
        for entry in entries {
            if entry.kind != mcp_vault_domain::FilesystemEntryKind::RegularFile {
                continue;
            }
            if let Some(path) = entry.path.filter(|path| legacy_path(core, path)) {
                paths.insert(path);
            }
        }
        for file in state.files().list_active_entries(context).await? {
            if file.entry_type == mcp_vault_state::EntryType::File && legacy_path(core, &file.path)
            {
                paths.insert(file.path);
            }
        }
        let mut files = Vec::new();
        for path in paths {
            files.push(FileToRetire {
                content_hash: if controlled_online {
                    core.managed_file_hash_during_maintenance(context, &path, permit)
                        .await
                } else {
                    core.managed_file_hash(context, &path).await
                }
                .map_err(|source| {
                    initialization_failure("hash", Some(path.clone()), 0, source.into())
                })?,
                path,
            });
        }
        let manifest = Manifest {
            schema: 1,
            files,
            original_counts: units.legacy_counts(context).await?,
            completed_files: 0,
        };
        units
            .begin_initialization(context, &json!(manifest))
            .await?;
        manifest
    };
    if manifest.schema != 1
        || manifest
            .files
            .iter()
            .any(|file| !legacy_path(core, &file.path))
    {
        return Err(MemoryError::InvalidInput(
            "memory initialization namespace is invalid",
        ));
    }
    let manifest_paths = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    core.discard_legacy_memory_journals_during_maintenance(context, permit, &manifest_paths)
        .await
        .map_err(|failure| {
            initialization_failure(
                "discard_journals",
                failure.path,
                saved_completed_files,
                failure.source.into(),
            )
        })?;
    // Every retry verifies the complete manifest again. Missing files are
    // idempotent; a new/different file at an old path fails its exact hash guard.
    for index in 0..manifest.files.len() {
        let file = &manifest.files[index];
        let record = state.files().get_active(context, &file.path).await?;
        core.retire_managed_file(
            context,
            &file.path,
            file.content_hash.as_deref(),
            Actor::system(),
            permit,
        )
        .await
        .map_err(|source| {
            initialization_failure("retire", Some(file.path.clone()), index, source.into())
        })?;
        if let Some(record) = record {
            state.index().remove_note(context, record.id).await?;
            state
                .providers()
                .delete_embeddings_for_object(context, "note", &record.id.to_string())
                .await?;
        }
        manifest.completed_files = index + 1;
        units
            .checkpoint_initialization(context, &json!(manifest))
            .await?;
    }
    let final_entries = if controlled_online {
        core.list_managed_files_during_maintenance(context, permit)
            .await
    } else {
        core.list_managed_files(context).await
    }
    .map_err(|source| {
        initialization_failure("final_scan", None, manifest.completed_files, source.into())
    })?;
    if final_entries
        .iter()
        .filter(|file| file.kind == mcp_vault_domain::FilesystemEntryKind::RegularFile)
        .filter_map(|file| file.path.as_ref())
        .any(|path| legacy_path(core, path))
    {
        return Err(MemoryError::Conflict);
    }
    units
        .finish_initialization(context, &json!(manifest))
        .await?;
    let remaining = units.legacy_counts(context).await?;
    if remaining.values().any(|count| *count != 0) {
        return Err(MemoryError::InvalidInput(
            "legacy memory cleanup is incomplete",
        ));
    }
    Ok(MemoryInitializationReport {
        vault_id: context.id(),
        vault_slug: context.slug().to_string(),
        outcome: "initialized".into(),
        file_count: manifest.files.len(),
        original_counts: manifest.original_counts,
        generation_paused: true,
    })
}
