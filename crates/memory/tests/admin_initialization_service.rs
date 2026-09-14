use mcp_vault_core::{CommitPhase, FailureInjector, VaultCore, VaultCoreRuntime};
use mcp_vault_domain::{
    Actor, MaintenanceGate, MaintenanceMode, Revision, SourcePlane, VaultContext, VaultId,
    VaultPath, VaultSlug,
};
use mcp_vault_memory::{
    InitializationStart, MemoryInitializationService, initialize_vault_memories_with_lease,
};
use mcp_vault_state::{
    CommitMutationInput, EntryType, FileOperation, NoopCommitHook, PrepareOperationInput,
    StateStore, VaultRecord, VaultStatus,
};
use mcp_vault_storage_fs::StorageOptions;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::io::AsyncReadExt;

#[path = "../../state/tests/support/memory_v3_cutover.rs"]
mod snapshot;

struct FailAt {
    phase: CommitPhase,
    fired: AtomicBool,
}

impl FailureInjector for FailAt {
    fn fail(&self, phase: CommitPhase) -> Result<(), &'static str> {
        if phase == self.phase && !self.fired.swap(true, Ordering::SeqCst) {
            Err("initialization recovery fixture fault")
        } else {
            Ok(())
        }
    }
}

async fn migrated_fixture(
    root: &std::path::Path,
) -> (
    StateStore,
    VaultRecord,
    VaultCore,
    MaintenanceGate,
    VaultPath,
    VaultPath,
    VaultPath,
) {
    let database = format!("sqlite://{}", root.join("state.sqlite3").display());
    snapshot::create_v27(&database).await;
    let state = StateStore::connect(&database).await.unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("main").unwrap(),
        root.join("main"),
        Revision::ZERO,
    )
    .unwrap();
    let vault = state
        .vaults()
        .insert(&context, "fixture", VaultStatus::Active)
        .await
        .unwrap();
    let gate = MaintenanceGate::new();
    let runtime = VaultCoreRuntime::new(gate.clone());
    let core = VaultCore::new(
        state.clone(),
        root.join("history"),
        Default::default(),
        StorageOptions::default(),
        runtime,
    );
    let ordinary = VaultPath::parse("ordinary.md").unwrap();
    core.create_bytes(
        &context,
        &ordinary,
        b"ordinary note survives initialization\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let old = core
        .managed_root()
        .join(&VaultPath::parse("memory/current/explicit/legacy.md").unwrap())
        .unwrap();
    let old_record = core
        .create_managed_bytes(
            &context,
            &old,
            b"legacy memory body\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    snapshot::seed_legacy(
        &database,
        &context.id().to_string(),
        &mcp_vault_domain::MemoryId::new().to_string(),
        &old_record.file.id.to_string(),
        old.as_str(),
    )
    .await;
    let new_v3 = core
        .managed_root()
        .join(&VaultPath::parse("memory-v3/explicit/preserved.md").unwrap())
        .unwrap();
    core.create_managed_bytes(
        &context,
        &new_v3,
        b"new v3 memory remains\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    state.close().await;

    let lock = StateStore::acquire_process_lock(&database).unwrap();
    let offline = StateStore::connect_offline_exclusive(&database)
        .await
        .unwrap();
    offline.migrate().await.unwrap();
    offline.close().await;
    drop(lock);
    let state = StateStore::connect(&database).await.unwrap();
    let vault = state.vaults().find_by_id(vault.id).await.unwrap().unwrap();
    let core = VaultCore::new(
        state.clone(),
        root.join("history"),
        Default::default(),
        StorageOptions::default(),
        VaultCoreRuntime::new(gate.clone()),
    );
    (state, vault, core, gate, ordinary, old, new_v3)
}

async fn fresh_fixture(
    root: &std::path::Path,
) -> (StateStore, VaultRecord, VaultCore, MaintenanceGate) {
    let database = format!("sqlite://{}", root.join("state.sqlite3").display());
    let state = StateStore::connect_and_migrate(&database).await.unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("fresh").unwrap(),
        root.join("fresh"),
        Revision::ZERO,
    )
    .unwrap();
    let vault = state
        .vaults()
        .insert(&context, "fresh fixture", VaultStatus::Active)
        .await
        .unwrap();
    let gate = MaintenanceGate::new();
    let core = VaultCore::new(
        state.clone(),
        root.join("history"),
        Default::default(),
        StorageOptions::default(),
        VaultCoreRuntime::new(gate.clone()),
    );
    let note = VaultPath::parse("fresh.md").unwrap();
    core.create_bytes(
        &context,
        &note,
        b"fresh content remains untouched\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    (state, vault, core, gate)
}

async fn wait_for_task(
    state: &StateStore,
    vault: &VaultRecord,
    expected: &str,
    gate: &MaintenanceGate,
    history_root: &std::path::Path,
    storage_options: StorageOptions,
) {
    let context = vault.context().unwrap();
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let task = state
                .memory_units()
                .initialization_task(&context)
                .await
                .unwrap();
            if task.as_ref().is_some_and(|task| task.state == expected) {
                return;
            }
            if let Some(task) = task
                && task.state == "failed"
                && expected != "failed"
            {
                let runtime = VaultCoreRuntime::new(gate.clone());
                let core = VaultCore::new(
                    state.clone(),
                    history_root.to_owned(),
                    Default::default(),
                    storage_options,
                    runtime.clone(),
                );
                let lease = gate
                    .try_begin_offline()
                    .expect("failed task should release its maintenance lease");
                let error = initialize_vault_memories_with_lease(
                    state,
                    &context,
                    &core,
                    &runtime.maintenance_recovery_permit(),
                    &lease,
                )
                .await
                .expect_err("diagnostic retry unexpectedly succeeded");
                panic!("initialization failed: {task:?}; direct error: {error:?}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("initialization task did not reach {expected}"));
}

#[tokio::test]
async fn admin_initialization_runs_in_background_and_preserves_current_data() {
    let directory = tempfile::tempdir().unwrap();
    let (state, vault, core, gate, ordinary, old, new_v3) =
        migrated_fixture(directory.path()).await;
    let service = MemoryInitializationService::new(
        state.clone(),
        gate.clone(),
        VaultCoreRuntime::new(gate.clone()),
        directory.path().join("history"),
        StorageOptions::default(),
    );
    let start = service.start(&vault, false).await.unwrap();
    assert!(matches!(start, InitializationStart::Accepted(_)));
    wait_for_task(
        &state,
        &vault,
        "ready",
        &gate,
        &directory.path().join("history"),
        StorageOptions::default(),
    )
    .await;
    assert_eq!(gate.mode(), MaintenanceMode::Normal);
    assert!(
        !state
            .memory_units()
            .initialization_required(&vault.context().unwrap())
            .await
            .unwrap()
    );
    assert!(
        core.read_managed(&vault.context().unwrap(), &old)
            .await
            .is_err()
    );
    assert!(
        core.read_managed(&vault.context().unwrap(), &new_v3)
            .await
            .is_ok()
    );
    let mut file = core
        .read(&vault.context().unwrap(), &ordinary)
        .await
        .unwrap();
    let mut body = Vec::new();
    file.reader.read_to_end(&mut body).await.unwrap();
    assert_eq!(body, b"ordinary note survives initialization\n");
    service.shutdown().await;
    state.close().await;
}

#[tokio::test]
async fn failed_initialization_is_offline_and_explicit_resume_reaches_ready() {
    let directory = tempfile::tempdir().unwrap();
    let (state, vault, _core, gate, _ordinary, old, _new_v3) =
        migrated_fixture(directory.path()).await;
    let blocked = MemoryInitializationService::new(
        state.clone(),
        gate.clone(),
        VaultCoreRuntime::new(gate.clone()),
        directory.path().join("history"),
        StorageOptions {
            minimum_free_bytes: u64::MAX,
            ..StorageOptions::default()
        },
    );
    assert!(matches!(
        blocked.start(&vault, false).await.unwrap(),
        InitializationStart::Accepted(_)
    ));
    wait_for_task(
        &state,
        &vault,
        "failed",
        &gate,
        &directory.path().join("history"),
        StorageOptions {
            minimum_free_bytes: u64::MAX,
            ..StorageOptions::default()
        },
    )
    .await;
    assert_eq!(gate.mode(), MaintenanceMode::Offline);
    let status = blocked.status(&vault).await.unwrap();
    assert_eq!(status["task"]["state"], "failed");
    assert!(blocked.start(&vault, false).await.is_err());

    let resumed = MemoryInitializationService::new(
        state.clone(),
        gate.clone(),
        VaultCoreRuntime::new(gate.clone()),
        directory.path().join("history"),
        StorageOptions::default(),
    );
    assert!(matches!(
        resumed.start(&vault, true).await.unwrap(),
        InitializationStart::Accepted(_)
    ));
    wait_for_task(
        &state,
        &vault,
        "ready",
        &gate,
        &directory.path().join("history"),
        StorageOptions::default(),
    )
    .await;
    assert_eq!(gate.mode(), MaintenanceMode::Normal);
    assert!(
        state
            .memory_units()
            .initialization(&vault.context().unwrap())
            .await
            .unwrap()
            .is_some_and(|row| row.phase == "ready")
    );
    let core = VaultCore::new(
        state.clone(),
        directory.path().join("history"),
        Default::default(),
        StorageOptions::default(),
        VaultCoreRuntime::new(gate.clone()),
    );
    assert!(
        core.read_managed(&vault.context().unwrap(), &old)
            .await
            .is_err()
    );
    resumed.shutdown().await;
    blocked.shutdown().await;
    state.close().await;
}

#[tokio::test]
async fn existing_operation_guard_delays_worker_until_it_is_released() {
    let directory = tempfile::tempdir().unwrap();
    let (state, vault, _core, gate, _ordinary, _old, _new_v3) =
        migrated_fixture(directory.path()).await;
    let service = MemoryInitializationService::new(
        state.clone(),
        gate.clone(),
        VaultCoreRuntime::new(gate.clone()),
        directory.path().join("history"),
        StorageOptions::default(),
    );
    let guard = gate.try_start_operation().expect("operation admission");
    assert!(matches!(
        service.start(&vault, false).await.unwrap(),
        InitializationStart::Accepted(_)
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_ne!(
        state
            .memory_units()
            .initialization_task(&vault.context().unwrap())
            .await
            .unwrap()
            .unwrap()
            .state,
        "ready"
    );
    drop(guard);
    wait_for_task(
        &state,
        &vault,
        "ready",
        &gate,
        &directory.path().join("history"),
        StorageOptions::default(),
    )
    .await;
    assert_eq!(gate.mode(), MaintenanceMode::Normal);
    service.shutdown().await;
    state.close().await;
}

#[tokio::test]
async fn fresh_vault_without_initialization_row_returns_ready_without_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let (state, vault, core, gate) = fresh_fixture(directory.path()).await;
    let service = MemoryInitializationService::new(
        state.clone(),
        gate.clone(),
        VaultCoreRuntime::new(gate.clone()),
        directory.path().join("history"),
        StorageOptions::default(),
    );
    let result = service.start(&vault, false).await.unwrap();
    assert!(matches!(result, InitializationStart::Ready(_)));
    assert!(
        state
            .memory_units()
            .initialization_task(&vault.context().unwrap())
            .await
            .unwrap()
            .is_none()
    );
    let note = VaultPath::parse("fresh.md").unwrap();
    let mut file = core.read(&vault.context().unwrap(), &note).await.unwrap();
    let mut body = Vec::new();
    file.reader.read_to_end(&mut body).await.unwrap();
    assert_eq!(body, b"fresh content remains untouched\n");
    assert_eq!(gate.mode(), MaintenanceMode::Normal);
    service.shutdown().await;
    state.close().await;
}

#[tokio::test]
async fn initialization_failure_diagnostics_survive_restart_and_clear_on_requeue() {
    let directory = tempfile::tempdir().unwrap();
    let (state, vault, _core, gate, _ordinary, old, _new_v3) =
        migrated_fixture(directory.path()).await;
    let blocked = MemoryInitializationService::new(
        state.clone(),
        gate.clone(),
        VaultCoreRuntime::new(gate.clone()),
        directory.path().join("history"),
        StorageOptions {
            minimum_free_bytes: u64::MAX,
            ..StorageOptions::default()
        },
    );
    assert!(matches!(
        blocked.start(&vault, false).await.unwrap(),
        InitializationStart::Accepted(_)
    ));
    wait_for_task(
        &state,
        &vault,
        "failed",
        &gate,
        &directory.path().join("history"),
        StorageOptions {
            minimum_free_bytes: u64::MAX,
            ..StorageOptions::default()
        },
    )
    .await;
    let context = vault.context().unwrap();
    let failed = state
        .memory_units()
        .initialization_task(&context)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.error_stage.as_deref(), Some("retire"));
    assert!(
        failed
            .error_code
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(
        failed
            .error_path
            .as_deref()
            .is_some_and(|value| value.ends_with(old.as_str()))
    );
    assert!(
        failed
            .error_source_code
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    );
    let persisted = (
        failed.error_code.clone(),
        failed.error_stage.clone(),
        failed.error_path.clone(),
        failed.error_source_code.clone(),
    );
    blocked.shutdown().await;
    state.close().await;

    let database = format!(
        "sqlite://{}",
        directory.path().join("state.sqlite3").display()
    );
    let reopened = StateStore::connect(&database).await.unwrap();
    let after_restart = reopened
        .memory_units()
        .initialization_task(&context)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            after_restart.error_code.clone(),
            after_restart.error_stage.clone(),
            after_restart.error_path.clone(),
            after_restart.error_source_code.clone(),
        ),
        persisted
    );
    let queued = reopened
        .memory_units()
        .queue_initialization_task(&context, "requeued-after-diagnostic", 1, true, "offline")
        .await
        .unwrap();
    assert_eq!(queued.state, "queued");
    assert!(queued.error_code.is_none());
    assert!(queued.error_stage.is_none());
    assert!(queued.error_path.is_none());
    assert!(queued.error_source_code.is_none());
    reopened.close().await;
}

#[tokio::test]
async fn legacy_memory_needs_review_is_discarded_after_confirmation() {
    let directory = tempfile::tempdir().unwrap();
    let (state, vault, core, gate, _ordinary, old, new_v3) =
        migrated_fixture(directory.path()).await;
    let context = vault.context().unwrap();

    let failing = core.clone().with_failure_injector(Arc::new(FailAt {
        phase: CommitPhase::RenameCommitted,
        fired: AtomicBool::new(false),
    }));
    assert!(matches!(
        failing
            .replace_managed_bytes(
                &context,
                &old,
                Revision::new(1),
                b"replacement legacy memory\n",
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await,
        Err(mcp_vault_core::VaultError::InjectedFailure(_))
    ));
    let journal = state
        .files()
        .list_incomplete(&context)
        .await
        .unwrap()
        .into_iter()
        .find(|journal| journal.destination_path.as_ref() == Some(&old))
        .expect("failed managed replace leaves a journal for the legacy path");
    state
        .files()
        .mark_needs_review(&context, journal.id, "legacy memory test review")
        .await
        .unwrap();

    let service = MemoryInitializationService::new(
        state.clone(),
        gate.clone(),
        VaultCoreRuntime::new(gate.clone()),
        directory.path().join("history"),
        StorageOptions::default(),
    );
    assert!(matches!(
        service.start(&vault, false).await.unwrap(),
        InitializationStart::Accepted(_)
    ));
    wait_for_task(
        &state,
        &vault,
        "ready",
        &gate,
        &directory.path().join("history"),
        StorageOptions::default(),
    )
    .await;

    let task = state
        .memory_units()
        .initialization_task(&context)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(task.state, "ready");
    assert!(task.error_code.is_none());
    assert_eq!(task.completed_files, 1);
    assert_eq!(
        state
            .memory_units()
            .initialization(&context)
            .await
            .unwrap()
            .unwrap()
            .phase,
        "ready"
    );
    let reviews = state.files().list_needs_review(&context).await.unwrap();
    assert!(reviews.is_empty());
    assert!(
        state
            .files()
            .list_incomplete(&context)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        state
            .files()
            .get_active(&context, &old)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        state
            .files()
            .get_active(&context, &new_v3)
            .await
            .unwrap()
            .is_some()
    );
    assert!(!context.content_root().join(old.as_str()).exists());

    service.shutdown().await;
    state.close().await;
}

#[tokio::test]
async fn explicit_initialization_discards_legacy_create_journals_without_replay() {
    let directory = tempfile::tempdir().unwrap();
    let (state, vault, core, gate, ordinary, old, new_v3) =
        migrated_fixture(directory.path()).await;
    let context = vault.context().unwrap();
    let legacy_row = state
        .files()
        .get_active(&context, &old)
        .await
        .unwrap()
        .unwrap();
    let hash = core
        .managed_file_hash(&context, &old)
        .await
        .unwrap()
        .unwrap();
    let size = std::fs::metadata(context.content_root().join(old.as_str()))
        .unwrap()
        .len();
    let operation_id = mcp_vault_domain::OperationId::new();
    let abandoned_file_id = mcp_vault_domain::FileId::new();
    let payload = serde_json::json!({
        "operation_id": operation_id,
        "file_id": abandoned_file_id,
        "entry_type": "file",
        "operation": "create",
        "path": old,
        "path_before": serde_json::Value::Null,
        "path_after": old,
        "expected_revision": serde_json::Value::Null,
        "require_absent": true,
        "tombstone_archive_path": serde_json::Value::Null,
        "content_hash": hash,
        "history_blob_hash": hash,
        "size": size,
        "modified_at": 1,
        "filesystem_identity": serde_json::Value::Null,
        "deleted_at": serde_json::Value::Null,
        "actor": Actor::system(),
        "source_plane": SourcePlane::System,
        "idempotency_key": serde_json::Value::Null,
        "audit_action": "file.create",
        "audit_metadata": serde_json::json!({}),
        "request_id": serde_json::Value::Null,
        "outbox_events": [],
    });
    state
        .files()
        .prepare_operation(
            &context,
            PrepareOperationInput {
                id: operation_id,
                operation: FileOperation::Create,
                source_path: Some(old.clone()),
                destination_path: Some(old.clone()),
                prior_file_id: Some(abandoned_file_id),
                expected_revision: None,
                prior_hash: None,
                proposed_hash: Some(hash.clone()),
                temp_path: None,
                payload,
                idempotency_key: None,
            },
        )
        .await
        .unwrap();
    state
        .files()
        .mark_file_committed(&context, operation_id, Some(&hash))
        .await
        .unwrap();

    // This models the previous recovery path: the old create now conflicts
    // with a current legacy record and becomes a reviewed replayed-create row.
    assert!(matches!(
        core.recover(&context).await,
        Err(mcp_vault_core::VaultError::AlreadyExists)
    ));
    let reviewed = state.files().list_needs_review(&context).await.unwrap();
    assert_eq!(reviewed.len(), 1);
    assert_eq!(reviewed[0].id, operation_id);

    let service = MemoryInitializationService::new(
        state.clone(),
        gate.clone(),
        VaultCoreRuntime::new(gate.clone()),
        directory.path().join("history"),
        StorageOptions::default(),
    );
    assert!(matches!(
        service.start(&vault, false).await.unwrap(),
        InitializationStart::Accepted(_)
    ));
    wait_for_task(
        &state,
        &vault,
        "ready",
        &gate,
        &directory.path().join("history"),
        StorageOptions::default(),
    )
    .await;

    assert!(
        state
            .files()
            .list_incomplete(&context)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        state
            .files()
            .list_needs_review(&context)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        state
            .files()
            .get_active(&context, &old)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        state
            .files()
            .get_active(&context, &new_v3)
            .await
            .unwrap()
            .is_some()
    );
    assert!(core.read(&context, &ordinary).await.is_ok());
    assert!(core.read_managed(&context, &new_v3).await.is_ok());
    assert!(!context.content_root().join(old.as_str()).exists());
    assert!(
        state
            .memory_units()
            .legacy_counts(&context)
            .await
            .unwrap()
            .values()
            .all(|count| *count == 0)
    );

    // The original operation record is kept as a terminal `discarded` fact;
    // it is no longer eligible for Core replay or operator review.
    let operation_summary = state.files().summarize_incomplete(&context).await.unwrap();
    assert!(operation_summary.is_empty());
    let _ = legacy_row;
    service.shutdown().await;
    state.close().await;
}

#[tokio::test]
async fn initialization_refuses_to_discard_a_journal_crossing_out_of_legacy_memory() {
    let directory = tempfile::tempdir().unwrap();
    let (state, vault, core, gate, _ordinary, old, _new_v3) =
        migrated_fixture(directory.path()).await;
    let context = vault.context().unwrap();
    let external_path = VaultPath::parse("ordinary-after-init.md").unwrap();
    let legacy_row = state
        .files()
        .get_active(&context, &old)
        .await
        .unwrap()
        .unwrap();
    let hash = core
        .managed_file_hash(&context, &old)
        .await
        .unwrap()
        .unwrap();
    let operation_id = mcp_vault_domain::OperationId::new();
    std::fs::rename(
        context.content_root().join(old.as_str()),
        context.content_root().join(external_path.as_str()),
    )
    .unwrap();
    // Core's public path policy rejects moving into/out of its managed root.
    // This durable row models a legacy/corrupt mixed-scope operation left by
    // an older writer; initialization must refuse to discard it silently.
    let payload = serde_json::json!({
        "operation_id": operation_id,
        "file_id": legacy_row.id,
        "entry_type": "file",
        "operation": "move",
        "path": external_path,
        "path_before": old,
        "path_after": external_path,
        "expected_revision": legacy_row.current_revision,
        "require_absent": true,
        "tombstone_archive_path": serde_json::Value::Null,
        "content_hash": hash,
        "history_blob_hash": hash,
        "size": legacy_row.size,
        "modified_at": 1,
        "filesystem_identity": serde_json::Value::Null,
        "deleted_at": serde_json::Value::Null,
        "actor": Actor::system(),
        "source_plane": SourcePlane::System,
        "idempotency_key": serde_json::Value::Null,
        "audit_action": "file.move",
        "audit_metadata": serde_json::json!({}),
        "request_id": serde_json::Value::Null,
        "outbox_events": [],
    });
    state
        .files()
        .prepare_operation(
            &context,
            PrepareOperationInput {
                id: operation_id,
                operation: FileOperation::Move,
                source_path: Some(old.clone()),
                destination_path: Some(external_path.clone()),
                prior_file_id: Some(legacy_row.id),
                expected_revision: Some(legacy_row.current_revision),
                prior_hash: Some(hash.clone()),
                proposed_hash: Some(hash.clone()),
                temp_path: None,
                payload,
                idempotency_key: None,
            },
        )
        .await
        .unwrap();
    state
        .files()
        .mark_file_committed(&context, operation_id, Some(&hash))
        .await
        .unwrap();
    let journal = state.files().list_incomplete(&context).await.unwrap();
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].source_path.as_ref(), Some(&old));
    assert_eq!(journal[0].destination_path.as_ref(), Some(&external_path));

    let service = MemoryInitializationService::new(
        state.clone(),
        gate.clone(),
        VaultCoreRuntime::new(gate.clone()),
        directory.path().join("history"),
        StorageOptions::default(),
    );
    assert!(matches!(
        service.start(&vault, false).await.unwrap(),
        InitializationStart::Accepted(_)
    ));
    wait_for_task(
        &state,
        &vault,
        "failed",
        &gate,
        &directory.path().join("history"),
        StorageOptions::default(),
    )
    .await;

    let task = state
        .memory_units()
        .initialization_task(&context)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(task.error_code.as_deref(), Some("memory_core_needs_review"));
    assert_eq!(task.error_stage.as_deref(), Some("discard_journals"));
    assert_eq!(task.error_path.as_deref(), Some(old.as_str()));
    assert_eq!(task.completed_files, 0);
    let incomplete = state.files().list_incomplete(&context).await.unwrap();
    assert_eq!(incomplete.len(), 1);
    assert_eq!(incomplete[0].id, journal[0].id);
    assert_eq!(
        std::fs::read(context.content_root().join(external_path.as_str())).unwrap(),
        b"legacy memory body\n"
    );
    assert!(!context.content_root().join(old.as_str()).exists());

    service.shutdown().await;
    state.close().await;
}

#[tokio::test]
async fn initialization_leaves_unrelated_review_journals_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let (state, vault, core, gate, ordinary, _old, new_v3) =
        migrated_fixture(directory.path()).await;
    let context = vault.context().unwrap();

    // A is a real staged Core create interrupted after the outbox phase.
    let replayed = VaultPath::parse("recovery/replayed.md").unwrap();
    let failing = core.clone().with_failure_injector(Arc::new(FailAt {
        phase: CommitPhase::OutboxInserted,
        fired: AtomicBool::new(false),
    }));
    let mut staged = failing
        .begin_put(
            &context,
            &replayed,
            true,
            true,
            Actor::system(),
            SourcePlane::WebDav,
        )
        .await
        .unwrap();
    staged.write_chunk(b"replayed").await.unwrap();
    assert!(staged.commit().await.is_err());
    let incomplete = state.files().list_incomplete(&context).await.unwrap();
    assert_eq!(incomplete.len(), 1);
    let staged_old = &incomplete[0];
    assert_eq!(staged_old.source_path.as_ref(), Some(&replayed));
    assert_eq!(staged_old.destination_path.as_ref(), Some(&replayed));
    assert!(staged_old.temp_path.is_some());
    let old_file_id = staged_old.prior_file_id.unwrap();
    let hash = staged_old.proposed_hash.clone().unwrap();
    let replacement_file_id = mcp_vault_domain::FileId::new();
    let replacement_operation = mcp_vault_domain::OperationId::new();
    let payload = serde_json::json!({
        "operation_id": replacement_operation,
        "file_id": replacement_file_id,
        "entry_type": "file",
        "operation": "create",
        "path": replayed,
        "path_before": serde_json::Value::Null,
        "path_after": replayed,
        "expected_revision": serde_json::Value::Null,
        "require_absent": true,
        "content_hash": hash,
        "history_blob_hash": hash,
        "size": 8,
        "modified_at": 1,
        "filesystem_identity": serde_json::Value::Null,
        "deleted_at": serde_json::Value::Null,
    });
    state
        .files()
        .prepare_operation(
            &context,
            PrepareOperationInput {
                id: replacement_operation,
                operation: FileOperation::Create,
                source_path: Some(replayed.clone()),
                destination_path: Some(replayed.clone()),
                prior_file_id: Some(replacement_file_id),
                expected_revision: None,
                prior_hash: None,
                proposed_hash: Some(hash.clone()),
                temp_path: Some(VaultPath::parse(".replayed-replacement.tmp").unwrap()),
                payload,
                idempotency_key: None,
            },
        )
        .await
        .unwrap();
    state
        .files()
        .mark_file_committed(&context, replacement_operation, Some(&hash))
        .await
        .unwrap();
    state
        .files()
        .commit_mutation(
            &context,
            CommitMutationInput {
                operation_id: replacement_operation,
                file_id: replacement_file_id,
                entry_type: EntryType::File,
                path: replayed.clone(),
                path_before: None,
                path_after: Some(replayed.clone()),
                expected_revision: None,
                require_absent: true,
                tombstone_archive_path: None,
                content_hash: Some(hash.clone()),
                history_blob_hash: Some(hash.clone()),
                size: 8,
                modified_at: 1,
                filesystem_identity: None,
                deleted_at: None,
                operation: FileOperation::Create,
                actor: Actor::system(),
                source_plane: SourcePlane::System,
                idempotency_key: None,
                audit_action: "test.replayed_create".to_owned(),
                audit_metadata: serde_json::json!({}),
                request_id: None,
                outbox_events: Vec::new(),
            },
            &NoopCommitHook,
        )
        .await
        .unwrap();

    // The ordinary-note create is deliberately held for review. Memory
    // initialization must not adjudicate or be blocked by this unrelated row.
    let ordinary_operation = incomplete[0].id;
    state
        .files()
        .mark_needs_review(&context, ordinary_operation, "ordinary note test review")
        .await
        .unwrap();

    let pending_ordinary = VaultPath::parse("智能体/基础知识/初始化前置要求和流程.md").unwrap();
    let failing = core.clone().with_failure_injector(Arc::new(FailAt {
        phase: CommitPhase::RenameCommitted,
        fired: AtomicBool::new(false),
    }));
    assert!(matches!(
        failing
            .create_bytes(
                &context,
                &pending_ordinary,
                b"ordinary note pending recovery\n",
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await,
        Err(mcp_vault_core::VaultError::InjectedFailure(_))
    ));

    // D is another real create, left with a committed canonical rename but no
    // metadata. Its path is in the predecessor namespace and must be retired.
    let staged_legacy = core
        .managed_root()
        .join(&VaultPath::parse("memory/current/explicit/recovered.md").unwrap())
        .unwrap();
    let failing = core.clone().with_failure_injector(Arc::new(FailAt {
        phase: CommitPhase::RenameCommitted,
        fired: AtomicBool::new(false),
    }));
    let error = failing
        .create_managed_bytes(
            &context,
            &staged_legacy,
            b"recovered legacy body\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        mcp_vault_core::VaultError::InjectedFailure(_)
    ));
    let incomplete_before_cleanup = state.files().list_incomplete(&context).await.unwrap();
    assert_eq!(incomplete_before_cleanup.len(), 2);
    assert!(
        incomplete_before_cleanup
            .iter()
            .any(|journal| { journal.destination_path.as_ref() == Some(&pending_ordinary) })
    );
    assert!(
        incomplete_before_cleanup
            .iter()
            .any(|journal| journal.destination_path.as_ref() == Some(&staged_legacy))
    );
    let reviews = state.files().list_needs_review(&context).await.unwrap();
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].id, ordinary_operation);

    state
        .vaults()
        .set_status(&context, VaultStatus::Error)
        .await
        .unwrap();
    let errored_vault = state.vaults().find_by_id(vault.id).await.unwrap().unwrap();
    let service = MemoryInitializationService::new(
        state.clone(),
        gate.clone(),
        VaultCoreRuntime::new(gate.clone()),
        directory.path().join("history"),
        StorageOptions::default(),
    );
    assert!(matches!(
        service.start(&errored_vault, false).await.unwrap(),
        InitializationStart::Accepted(_)
    ));
    wait_for_task(
        &state,
        &errored_vault,
        "ready",
        &gate,
        &directory.path().join("history"),
        StorageOptions::default(),
    )
    .await;
    assert_eq!(gate.mode(), MaintenanceMode::Normal);
    assert_eq!(
        state
            .vaults()
            .find_by_id(vault.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        VaultStatus::Error
    );
    let incomplete_after_cleanup = state.files().list_incomplete(&context).await.unwrap();
    assert_eq!(incomplete_after_cleanup.len(), 1);
    assert_eq!(
        incomplete_after_cleanup[0].destination_path.as_ref(),
        Some(&pending_ordinary)
    );
    let reviews = state.files().list_needs_review(&context).await.unwrap();
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].id, ordinary_operation);
    assert!(core.read(&context, &replayed).await.is_ok());
    assert!(core.read_managed(&context, &staged_legacy).await.is_err());
    assert!(
        !core
            .history(&context, &staged_legacy)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(core.read(&context, &ordinary).await.is_ok());
    assert!(core.read_managed(&context, &new_v3).await.is_ok());
    assert!(
        state
            .files()
            .get_by_id(&context, old_file_id)
            .await
            .unwrap()
            .is_none()
    );
    service.shutdown().await;
    state.close().await;
}
