use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use mcp_vault_core::{CommitPhase, FailureInjector, VaultCore, VaultError};
use mcp_vault_domain::{
    Actor, ActorType, Revision, SourcePlane, VaultContext, VaultId, VaultPath, VaultPathPolicy,
    VaultSlug,
};
use mcp_vault_state::{StateStore, VaultStatus};
use mcp_vault_storage_fs::{DestinationPolicy, DurabilityPolicy, StorageOptions, VaultStorage};
use tempfile::TempDir;
use tokio::io::AsyncReadExt;

fn path(value: &str) -> VaultPath {
    VaultPath::parse(value).unwrap()
}

async fn setup() -> (TempDir, StateStore, VaultContext, VaultCore) {
    let directory = tempfile::tempdir().unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("default").unwrap(),
        directory.path().join("content"),
        Revision::ZERO,
    )
    .unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    state
        .vaults()
        .insert(&context, "Default", VaultStatus::Active)
        .await
        .unwrap();
    let core = VaultCore::new(
        state.clone(),
        directory.path().join("history"),
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
        Default::default(),
    );
    (directory, state, context, core)
}

async fn read_bytes(core: &VaultCore, context: &VaultContext, path: &VaultPath) -> Vec<u8> {
    let mut result = core.read(context, path).await.unwrap();
    let mut bytes = Vec::new();
    result.reader.read_to_end(&mut bytes).await.unwrap();
    bytes
}

fn system_actor() -> Actor {
    Actor::system()
}

fn reconciler_actor() -> Actor {
    Actor::new(ActorType::Reconciler, None)
}

#[tokio::test]
async fn create_replace_append_exact_patch_and_history_share_one_core_path() {
    let (_directory, _state, context, core) = setup().await;
    let note = path("notes/design.md");
    let created = core
        .create_bytes(
            &context,
            &note,
            b"# Design\nold\n",
            system_actor(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    assert_eq!(created.revision.revision, Revision::new(1));
    assert_eq!(read_bytes(&core, &context, &note).await, b"# Design\nold\n");

    let replaced = core
        .replace_bytes(
            &context,
            &note,
            Revision::new(1),
            b"# Design\nnew\n",
            system_actor(),
            SourcePlane::Mcp,
            None,
        )
        .await
        .unwrap();
    assert_eq!(replaced.revision.revision, Revision::new(2));

    let stale = core
        .replace_bytes(
            &context,
            &note,
            Revision::new(1),
            b"stale\n",
            system_actor(),
            SourcePlane::Mcp,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(stale, VaultError::RevisionConflict { .. }));

    let appended = core
        .append_bytes(
            &context,
            &note,
            Revision::new(2),
            b"tail\n",
            system_actor(),
            SourcePlane::WebDav,
            None,
        )
        .await
        .unwrap();
    let patched = core
        .patch_unified_diff(
            &context,
            &note,
            appended.revision.revision,
            "--- a/notes/design.md\n+++ b/notes/design.md\n@@ -1,3 +1,3 @@\n # Design\n-new\n+final\n tail\n",
            system_actor(),
            SourcePlane::Mcp,
            None,
        )
        .await
        .unwrap();
    assert_eq!(patched.revision.revision, Revision::new(4));
    assert_eq!(
        read_bytes(&core, &context, &note).await,
        b"# Design\nfinal\ntail\n"
    );

    let history = core.history(&context, &note).await.unwrap();
    assert_eq!(history.len(), 4);
    assert!(
        history
            .iter()
            .all(|revision| revision.history_blob_hash.is_some())
    );
}

#[tokio::test]
async fn historical_reads_are_served_by_core_without_replacing_current_content() {
    let (_directory, _state, context, core) = setup().await;
    let note = path("history.md");
    let created = core
        .create_bytes(
            &context,
            &note,
            b"first\n",
            system_actor(),
            SourcePlane::Mcp,
            None,
        )
        .await
        .unwrap();
    core.replace_bytes(
        &context,
        &note,
        created.revision.revision,
        b"second\n",
        system_actor(),
        SourcePlane::Mcp,
        None,
    )
    .await
    .unwrap();

    let mut historical = core
        .read_revision(&context, &note, created.revision.revision)
        .await
        .unwrap();
    let mut historical_bytes = Vec::new();
    historical
        .reader
        .read_to_end(&mut historical_bytes)
        .await
        .unwrap();
    assert_eq!(historical_bytes, b"first\n");
    assert_eq!(read_bytes(&core, &context, &note).await, b"second\n");
}

#[tokio::test]
async fn move_copy_delete_and_restore_preserve_history_boundaries() {
    let (_directory, _state, context, core) = setup().await;
    let source = path("source.md");
    let moved = path("archive/moved.md");
    let copied = path("copy.md");
    let created = core
        .create_bytes(
            &context,
            &source,
            b"original\n",
            system_actor(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let moved_result = core
        .move_entry(
            &context,
            &source,
            &moved,
            created.revision.revision,
            system_actor(),
            SourcePlane::WebDav,
            None,
        )
        .await
        .unwrap();
    assert_eq!(moved_result.file.id, created.file.id);

    let copied_result = core
        .copy(
            &context,
            &moved,
            &copied,
            system_actor(),
            SourcePlane::Mcp,
            None,
        )
        .await
        .unwrap();
    assert_ne!(copied_result.file.id, moved_result.file.id);
    let deleted = core
        .delete(
            &context,
            &copied,
            copied_result.revision.revision,
            system_actor(),
            SourcePlane::Mcp,
            None,
        )
        .await
        .unwrap();
    assert!(deleted.file.deleted_at.is_some());
    let restored = core
        .restore(
            &context,
            &copied,
            Revision::new(1),
            deleted.revision.revision,
            system_actor(),
            SourcePlane::Mcp,
            None,
        )
        .await
        .unwrap();
    assert_eq!(restored.revision.operation.as_str(), "restore");
    assert!(restored.file.is_active());
    assert_eq!(read_bytes(&core, &context, &copied).await, b"original\n");
}

#[tokio::test]
async fn idempotency_replays_committed_result_and_rejects_key_reuse() {
    let (_directory, state, context, core) = setup().await;
    let first = core
        .create_bytes(
            &context,
            &path("idempotent.md"),
            b"same",
            system_actor(),
            SourcePlane::Mcp,
            Some("client-key"),
        )
        .await
        .unwrap();
    let replay = core
        .create_bytes(
            &context,
            &path("idempotent.md"),
            b"same",
            system_actor(),
            SourcePlane::Mcp,
            Some("client-key"),
        )
        .await
        .unwrap();
    assert_eq!(replay, first);
    assert_eq!(
        state
            .files()
            .count_outbox_events(&context, &first.file.id.to_string())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        state
            .files()
            .count_audit_entries(&context, &first.file.id.to_string())
            .await
            .unwrap(),
        1
    );

    let conflict = core
        .create_bytes(
            &context,
            &path("other.md"),
            b"same",
            system_actor(),
            SourcePlane::Mcp,
            Some("client-key"),
        )
        .await
        .unwrap_err();
    assert!(matches!(conflict, VaultError::IdempotencyConflict));
}

#[tokio::test]
async fn invalid_patch_is_exactly_rejected_without_a_new_revision() {
    let (_directory, _state, context, core) = setup().await;
    let note = path("exact.md");
    let created = core
        .create_bytes(
            &context,
            &note,
            b"one\ntwo\n",
            system_actor(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let error = core
        .patch_unified_diff(
            &context,
            &note,
            created.revision.revision,
            "@@ -1,1 +1,1 @@\n-wrong\n+three\n",
            system_actor(),
            SourcePlane::Mcp,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, VaultError::InvalidPatch(_)));
    assert_eq!(read_bytes(&core, &context, &note).await, b"one\ntwo\n");
}

#[tokio::test]
async fn staged_put_and_directory_services_keep_core_metadata_consistent() {
    let (_directory, state, context, core) = setup().await;
    let note = path("notes/staged.md");
    let mut staged = core
        .begin_put(
            &context,
            &note,
            true,
            false,
            system_actor(),
            SourcePlane::WebDav,
        )
        .await
        .unwrap();
    staged.write_chunk(b"streamed ").await.unwrap();
    staged.write_chunk(b"through Core").await.unwrap();
    let result = staged.commit().await.unwrap();
    assert_eq!(result.file.current_revision, Revision::new(1));
    assert_eq!(
        read_bytes(&core, &context, &note).await,
        b"streamed through Core"
    );

    let metadata = core.metadata(&context, &note).await.unwrap();
    assert_eq!(metadata.metadata.size, 21);
    assert_eq!(metadata.etag, result.etag.trim_matches('"'));
    let directory = path("empty");
    core.create_directory(&context, &directory, system_actor(), SourcePlane::WebDav)
        .await
        .unwrap();
    let root_entries = core
        .list_directory(&context, &VaultPath::root())
        .await
        .unwrap();
    assert!(root_entries.iter().any(|entry| {
        entry.metadata.path.as_ref() == Some(&directory)
            && entry.metadata.kind == mcp_vault_domain::FilesystemEntryKind::Directory
    }));
    assert!(
        state
            .files()
            .get_active(&context, &directory)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn dropped_staged_put_rolls_back_its_journal_and_temp_payload() {
    let (_directory, state, context, core) = setup().await;
    let note = path("notes/interrupted.md");
    let mut staged = core
        .begin_put(
            &context,
            &note,
            true,
            false,
            system_actor(),
            SourcePlane::WebDav,
        )
        .await
        .unwrap();
    staged.write_chunk(b"partial upload").await.unwrap();
    drop(staged);

    for _ in 0..20 {
        if state
            .files()
            .list_incomplete(&context)
            .await
            .unwrap()
            .is_empty()
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        state
            .files()
            .list_incomplete(&context)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        core.metadata(&context, &note).await,
        Err(VaultError::NotFound)
    ));
}

#[tokio::test]
async fn out_of_band_content_change_is_diagnosed_before_core_read() {
    let (directory, _state, context, core) = setup().await;
    let note = path("external.md");
    core.create_bytes(
        &context,
        &note,
        b"canonical",
        system_actor(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();

    let storage = VaultStorage::new(
        &context,
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
    );
    storage
        .write_bytes(
            &note,
            b"outside mutation",
            DestinationPolicy::ReplaceExisting,
        )
        .await
        .unwrap();
    assert!(matches!(
        core.read(&context, &note).await,
        Err(VaultError::ExternalMismatch)
    ));
    assert!(directory.path().join("content/external.md").is_file());
}

#[tokio::test]
async fn reconciliation_imports_direct_create_edit_delete_and_restore_as_external_revisions() {
    let (directory, state, context, core) = setup().await;
    let note = path("external.md");
    std::fs::create_dir_all(directory.path().join("content")).unwrap();
    std::fs::write(
        directory.path().join("content/external.md"),
        b"created outside",
    )
    .unwrap();

    let first = core.reconcile(&context, reconciler_actor()).await.unwrap();
    assert_eq!(first.imported, 1);
    assert_eq!(read_bytes(&core, &context, &note).await, b"created outside");
    let file = state
        .files()
        .get_active(&context, &note)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(file.current_revision, Revision::new(1));
    let events = state
        .outbox()
        .find_by_aggregate(&context, &file.id.to_string())
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "FileCreated");
    assert_eq!(events[0].payload["operation"], "external_change");
    assert_eq!(
        state
            .files()
            .list_revisions(&context, file.id)
            .await
            .unwrap()[0]
            .operation,
        mcp_vault_state::FileOperation::ExternalChange
    );

    std::fs::write(
        directory.path().join("content/external.md"),
        b"edited outside",
    )
    .unwrap();
    let second = core.reconcile(&context, reconciler_actor()).await.unwrap();
    assert_eq!(second.imported, 1);
    let file = state
        .files()
        .get_active(&context, &note)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(file.current_revision, Revision::new(2));
    let events = state
        .outbox()
        .find_by_aggregate(&context, &file.id.to_string())
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].event_type, "FileUpdated");
    assert_eq!(events[1].payload["operation"], "external_change");

    std::fs::remove_file(directory.path().join("content/external.md")).unwrap();
    let third = core.reconcile(&context, reconciler_actor()).await.unwrap();
    assert_eq!(third.deleted, 1);
    assert!(
        state
            .files()
            .get_active(&context, &note)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        state
            .files()
            .get_any_by_path(&context, &note)
            .await
            .unwrap()
            .unwrap()
            .deleted_at
            .is_some()
    );
    let events = state
        .outbox()
        .find_by_aggregate(&context, &file.id.to_string())
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[2].event_type, "FileDeleted");
    assert_eq!(events[2].payload["operation"], "external_change");

    std::fs::write(
        directory.path().join("content/external.md"),
        b"restored outside",
    )
    .unwrap();
    let fourth = core.reconcile(&context, reconciler_actor()).await.unwrap();
    assert_eq!(fourth.imported, 1);
    let restored = state
        .files()
        .get_active(&context, &note)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.id, file.id);
    assert_eq!(restored.current_revision, Revision::new(4));
    let events = state
        .outbox()
        .find_by_aggregate(&context, &file.id.to_string())
        .await
        .unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events[3].event_type, "FileRestored");
    assert_eq!(events[3].payload["operation"], "external_change");
}

#[cfg(unix)]
#[tokio::test]
async fn reconciliation_preserves_file_identity_for_an_external_move() {
    let (directory, state, context, core) = setup().await;
    let source = path("before.md");
    let destination = path("after.md");
    let created = core
        .create_bytes(
            &context,
            &source,
            b"moved outside",
            system_actor(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();

    std::fs::rename(
        directory.path().join("content/before.md"),
        directory.path().join("content/after.md"),
    )
    .unwrap();
    let report = core.reconcile(&context, reconciler_actor()).await.unwrap();
    assert_eq!(report.moved, 1);
    assert_eq!(report.imported, 0);
    assert_eq!(report.deleted, 0);

    assert!(
        state
            .files()
            .get_active(&context, &source)
            .await
            .unwrap()
            .is_none()
    );
    let moved = state
        .files()
        .get_active(&context, &destination)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(moved.id, created.file.id);
    assert_eq!(moved.current_revision, Revision::new(2));
    assert_eq!(
        state
            .files()
            .list_revisions(&context, moved.id)
            .await
            .unwrap()[1]
            .operation,
        mcp_vault_state::FileOperation::Move
    );
}

#[cfg(unix)]
#[tokio::test]
async fn reconciliation_does_not_turn_an_unsafe_symlink_into_a_delete() {
    let (directory, state, context, core) = setup().await;
    let note = path("protected.md");
    core.create_bytes(
        &context,
        &note,
        b"canonical",
        system_actor(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    std::fs::remove_file(directory.path().join("content/protected.md")).unwrap();
    std::os::unix::fs::symlink(
        directory.path().join("content"),
        directory.path().join("content/protected.md"),
    )
    .unwrap();

    let report = core.reconcile(&context, reconciler_actor()).await.unwrap();
    assert!(report.unsafe_entries_skipped >= 1);
    assert!(report.missing_deletes_skipped);
    assert!(
        state
            .files()
            .get_active(&context, &note)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn two_vault_contexts_are_isolated_in_core_state_and_storage() {
    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let first = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("first").unwrap(),
        directory.path().join("first"),
        Revision::ZERO,
    )
    .unwrap();
    let second = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("second").unwrap(),
        directory.path().join("second"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&first, "First", VaultStatus::Active)
        .await
        .unwrap();
    state
        .vaults()
        .insert(&second, "Second", VaultStatus::Active)
        .await
        .unwrap();
    let core = VaultCore::new(
        state,
        directory.path().join("history"),
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
        Default::default(),
    );
    core.create_bytes(
        &first,
        &path("same.md"),
        b"first",
        system_actor(),
        SourcePlane::Mcp,
        None,
    )
    .await
    .unwrap();
    core.create_bytes(
        &second,
        &path("same.md"),
        b"second",
        system_actor(),
        SourcePlane::Mcp,
        None,
    )
    .await
    .unwrap();
    assert_eq!(read_bytes(&core, &first, &path("same.md")).await, b"first");
    assert_eq!(
        read_bytes(&core, &second, &path("same.md")).await,
        b"second"
    );
}

#[tokio::test]
async fn concurrent_exact_replacements_serialize_and_one_conflicts() {
    let (_directory, _state, context, core) = setup().await;
    let note = path("race.md");
    let created = core
        .create_bytes(
            &context,
            &note,
            b"base",
            system_actor(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let first_core = core.clone();
    let second_core = core.clone();
    let first_context = context.clone();
    let second_context = context.clone();
    let (first, second) = tokio::join!(
        first_core.replace_bytes(
            &first_context,
            &note,
            created.revision.revision,
            b"first",
            system_actor(),
            SourcePlane::Mcp,
            None,
        ),
        second_core.replace_bytes(
            &second_context,
            &note,
            created.revision.revision,
            b"second",
            system_actor(),
            SourcePlane::Mcp,
            None,
        )
    );
    assert!(first.is_ok() ^ second.is_ok());
    assert!(
        matches!(first, Err(VaultError::RevisionConflict { .. }))
            || matches!(second, Err(VaultError::RevisionConflict { .. }))
    );
}

struct FailAt {
    phase: CommitPhase,
    fired: AtomicBool,
}

impl FailAt {
    fn new(phase: CommitPhase) -> Self {
        Self {
            phase,
            fired: AtomicBool::new(false),
        }
    }
}

impl FailureInjector for FailAt {
    fn fail(&self, phase: CommitPhase) -> Result<(), &'static str> {
        if phase == self.phase && !self.fired.swap(true, Ordering::SeqCst) {
            Err("deterministic recovery fault")
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn injected_phases_recover_to_old_or_new_atomic_state() {
    let phases = [
        CommitPhase::JournalPrepared,
        CommitPhase::TempFileWritten,
        CommitPhase::FileFsynced,
        CommitPhase::RenameCommitted,
        CommitPhase::MetadataTransactionStarted,
        CommitPhase::OutboxInserted,
        CommitPhase::MetadataCommitted,
    ];
    for (index, phase) in phases.into_iter().enumerate() {
        let (directory, state, context, core) = setup().await;
        let failing = core.with_failure_injector(Arc::new(FailAt::new(phase)));
        let note = path(&format!("fault-{index}.md"));
        let error = failing
            .create_bytes(
                &context,
                &note,
                b"recovered",
                system_actor(),
                SourcePlane::Mcp,
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            VaultError::InjectedFailure(_) | VaultError::State(_)
        ));

        let recovery_core = VaultCore::new(
            state,
            directory.path().join("history"),
            VaultPathPolicy::default(),
            StorageOptions {
                durability: DurabilityPolicy::None,
                minimum_free_bytes: 0,
                ..StorageOptions::default()
            },
            Default::default(),
        );
        let report = recovery_core.recover(&context).await.unwrap();
        assert_eq!(report.needs_review, 0, "phase {}", phase.as_str());
        if phase == CommitPhase::MetadataCommitted {
            assert_eq!(report.finalized, 0, "phase {}", phase.as_str());
            assert_eq!(report.rolled_back, 0, "phase {}", phase.as_str());
            assert_eq!(
                read_bytes(&recovery_core, &context, &note).await,
                b"recovered"
            );
        } else if phase == CommitPhase::RenameCommitted
            || phase == CommitPhase::MetadataTransactionStarted
            || phase == CommitPhase::OutboxInserted
        {
            assert_eq!(report.finalized, 1, "phase {}", phase.as_str());
            assert_eq!(
                read_bytes(&recovery_core, &context, &note).await,
                b"recovered"
            );
        } else {
            assert_eq!(report.rolled_back, 1, "phase {}", phase.as_str());
            assert!(matches!(
                recovery_core.read(&context, &note).await,
                Err(VaultError::NotFound)
            ));
        }
    }
}

#[tokio::test]
async fn recovery_removes_a_linked_temporary_name_after_canonical_install() {
    let (directory, state, context, core) = setup().await;
    let failing = core.with_failure_injector(Arc::new(FailAt::new(CommitPhase::RenameCommitted)));
    let note = path("linked-recovery.md");
    let error = failing
        .create_bytes(
            &context,
            &note,
            b"complete linked payload",
            system_actor(),
            SourcePlane::Mcp,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, VaultError::InjectedFailure(_)));

    let journals = state.files().list_incomplete(&context).await.unwrap();
    assert_eq!(journals.len(), 1);
    let temporary = journals[0].temp_path.as_ref().unwrap();
    let target_path = context.content_root().join(note.as_str());
    let temporary_path = context.content_root().join(temporary.as_str());

    // Model a crash after the fallback link is installed but before its
    // private temporary name is removed. Both names reference the same
    // already-complete inode.
    std::fs::hard_link(&target_path, &temporary_path).unwrap();
    assert!(temporary_path.exists());

    let recovery_core = VaultCore::new(
        state,
        directory.path().join("history"),
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
        Default::default(),
    );
    let report = recovery_core.recover(&context).await.unwrap();
    assert_eq!(report.finalized, 1);
    assert_eq!(report.needs_review, 0);
    assert!(!temporary_path.exists());
    assert_eq!(
        read_bytes(&recovery_core, &context, &note).await,
        b"complete linked payload"
    );
}

#[tokio::test]
async fn managed_memory_writes_are_atomic_hidden_and_reconciliation_safe() {
    let (directory, state, context, core) = setup().await;
    let managed = path("_mcp-vault/memory/records/2026/08/00000000-0000-7000-8000-000000000001.md");
    let created = core
        .create_managed_bytes(
            &context,
            &managed,
            b"---\nstatus: active\n---\nmanaged",
            system_actor(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let mut read = core.read_managed(&context, &managed).await.unwrap();
    let mut bytes = Vec::new();
    read.reader.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(bytes, b"---\nstatus: active\n---\nmanaged");
    assert!(matches!(
        core.read(&context, &managed).await,
        Err(VaultError::Domain(_))
    ));

    let report = core.reconcile(&context, reconciler_actor()).await.unwrap();
    assert_eq!(report.deleted, 0);
    assert!(
        state
            .files()
            .get_active(&context, &managed)
            .await
            .unwrap()
            .is_some()
    );

    let replaced = core
        .replace_managed_bytes(
            &context,
            &managed,
            created.file.current_revision,
            b"---\nstatus: archived\n---\nmanaged",
            system_actor(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    assert!(replaced.file.current_revision > created.file.current_revision);

    let failing = core.with_failure_injector(Arc::new(FailAt::new(CommitPhase::RenameCommitted)));
    let managed_two =
        path("_mcp-vault/memory/records/2026/08/00000000-0000-7000-8000-000000000002.md");
    assert!(
        failing
            .create_managed_bytes(
                &context,
                &managed_two,
                b"---\nstatus: active\n---\nrecovered",
                system_actor(),
                SourcePlane::System,
                None,
            )
            .await
            .is_err()
    );
    let recovery = VaultCore::new(
        state,
        directory.path().join("history"),
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
        Default::default(),
    );
    let recovered = recovery.recover(&context).await.unwrap();
    assert_eq!(recovered.needs_review, 0);
    assert!(recovery.read_managed(&context, &managed_two).await.is_ok());
}
