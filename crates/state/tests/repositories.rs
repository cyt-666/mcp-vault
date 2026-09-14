use std::path::PathBuf;

use mcp_vault_domain::{
    ActorId, ActorType, DomainError, FileId, OperationId, Revision, SourcePlane, VaultContext,
    VaultId, VaultPath, VaultSlug, WritePrecondition,
};
use mcp_vault_state::{
    CommitMutationInput, EntryType, FileOperation, NoopCommitHook, PrepareOperationInput,
    StateStore, SupersedeCreateResult, SupersedeCreateWitness, VaultAvailability, VaultRepository,
    VaultStatus,
};
use serde_json::json;

async fn store() -> StateStore {
    StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap()
}

fn context(slug: &str, root: &str) -> VaultContext {
    VaultContext::new(
        VaultId::new(),
        VaultSlug::new(slug).unwrap(),
        PathBuf::from(root),
        Revision::new(1),
    )
    .unwrap()
}

async fn insert(repository: &VaultRepository, context: &VaultContext) {
    repository
        .insert(context, context.slug().as_str(), VaultStatus::Active)
        .await
        .unwrap();
}

fn create_payload(
    operation_id: OperationId,
    file_id: FileId,
    path: &VaultPath,
    hash: &str,
    size: u64,
) -> serde_json::Value {
    serde_json::json!({
        "operation_id": operation_id,
        "file_id": file_id,
        "entry_type": "file",
        "operation": "create",
        "path": path,
        "path_before": serde_json::Value::Null,
        "path_after": path,
        "expected_revision": serde_json::Value::Null,
        "prior_hash": serde_json::Value::Null,
        "require_absent": true,
        "content_hash": hash,
        "size": size,
        "deleted_at": serde_json::Value::Null,
    })
}

async fn replayed_create_fixture() -> (
    StateStore,
    VaultContext,
    VaultPath,
    FileId,
    FileId,
    OperationId,
    OperationId,
    String,
) {
    let store = store().await;
    let context = context("witness", "/srv/witness");
    insert(&store.vaults(), &context).await;
    let path = VaultPath::parse("_mcp-vault/memory/current/facts/fact.md").unwrap();
    let hash = "a".repeat(64);
    let size = 7_u64;
    let old_file = FileId::new();
    let old_operation = OperationId::new();
    store
        .files()
        .prepare_operation(
            &context,
            PrepareOperationInput {
                id: old_operation,
                operation: FileOperation::Create,
                source_path: Some(path.clone()),
                destination_path: Some(path.clone()),
                prior_file_id: Some(old_file),
                expected_revision: None,
                prior_hash: None,
                proposed_hash: Some(hash.clone()),
                temp_path: Some(VaultPath::parse("_mcp-vault/.tmp/fact").unwrap()),
                payload: create_payload(old_operation, old_file, &path, &hash, size),
                idempotency_key: None,
            },
        )
        .await
        .unwrap();
    store
        .files()
        .mark_file_committed(&context, old_operation, Some(&hash))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let replacement_file = FileId::new();
    let replacement_operation = OperationId::new();
    store
        .files()
        .prepare_operation(
            &context,
            PrepareOperationInput {
                id: replacement_operation,
                operation: FileOperation::Create,
                source_path: Some(path.clone()),
                destination_path: Some(path.clone()),
                prior_file_id: Some(replacement_file),
                expected_revision: None,
                prior_hash: None,
                proposed_hash: Some(hash.clone()),
                temp_path: Some(VaultPath::parse("_mcp-vault/.tmp/replacement").unwrap()),
                payload: create_payload(
                    replacement_operation,
                    replacement_file,
                    &path,
                    &hash,
                    size,
                ),
                idempotency_key: None,
            },
        )
        .await
        .unwrap();
    store
        .files()
        .mark_file_committed(&context, replacement_operation, Some(&hash))
        .await
        .unwrap();
    store
        .files()
        .commit_mutation(
            &context,
            CommitMutationInput {
                operation_id: replacement_operation,
                file_id: replacement_file,
                entry_type: EntryType::File,
                path: path.clone(),
                path_before: None,
                path_after: Some(path.clone()),
                expected_revision: None,
                require_absent: true,
                tombstone_archive_path: None,
                content_hash: Some(hash.clone()),
                history_blob_hash: None,
                size,
                modified_at: 1,
                filesystem_identity: None,
                deleted_at: None,
                operation: FileOperation::Create,
                actor: mcp_vault_domain::Actor::new(ActorType::System, None),
                source_plane: SourcePlane::System,
                idempotency_key: None,
                audit_action: "test.create".to_owned(),
                audit_metadata: serde_json::json!({}),
                request_id: None,
                outbox_events: Vec::new(),
            },
            &NoopCommitHook,
        )
        .await
        .unwrap();
    (
        store,
        context,
        path,
        old_file,
        replacement_file,
        old_operation,
        replacement_operation,
        hash,
    )
}

#[tokio::test]
async fn replayed_create_witness_requires_real_source_and_temp_shape_and_is_idempotent() {
    let (
        store,
        context,
        path,
        old_file,
        replacement_file,
        old_operation,
        _replacement_operation,
        hash,
    ) = replayed_create_fixture().await;
    let witness = SupersedeCreateWitness {
        operation_id: old_operation,
        path,
        prior_file_id: old_file,
        replacement_file_id: replacement_file,
        replacement_revision: Revision::new(1),
        physical_hash: hash,
    };
    assert_eq!(
        store
            .files()
            .supersede_replayed_create(&context, &witness)
            .await
            .unwrap(),
        SupersedeCreateResult::Applied
    );
    assert_eq!(
        store
            .files()
            .supersede_replayed_create(&context, &witness)
            .await
            .unwrap(),
        SupersedeCreateResult::AlreadyApplied
    );
    let journal = store.files().list_incomplete(&context).await.unwrap();
    assert!(journal.is_empty());
    let audits = store
        .audit()
        .list_for_vault(&context, Some("file.recovery_superseded"), None, 20, 0)
        .await
        .unwrap();
    assert_eq!(audits.len(), 1);
    assert!(
        audits[0]
            .metadata
            .get("replacement_operation_id")
            .and_then(serde_json::Value::as_str)
            .is_some()
    );
}

#[tokio::test]
async fn explicit_legacy_memory_discard_terminalizes_only_selected_journals() {
    let (
        store,
        context,
        _path,
        _old_file,
        _replacement_file,
        old_operation,
        _replacement_operation,
        _hash,
    ) = replayed_create_fixture().await;
    let incomplete = store.files().list_incomplete(&context).await.unwrap();
    let selected = incomplete
        .iter()
        .find(|journal| journal.id == old_operation)
        .unwrap()
        .clone();
    let unrelated_operation = OperationId::new();
    let unrelated_file = FileId::new();
    let unrelated_path = VaultPath::parse("ordinary.md").unwrap();
    let unrelated_hash = "b".repeat(64);
    store
        .files()
        .prepare_operation(
            &context,
            PrepareOperationInput {
                id: unrelated_operation,
                operation: FileOperation::Create,
                source_path: Some(unrelated_path.clone()),
                destination_path: Some(unrelated_path.clone()),
                prior_file_id: Some(unrelated_file),
                expected_revision: None,
                prior_hash: None,
                proposed_hash: Some(unrelated_hash.clone()),
                temp_path: None,
                payload: create_payload(
                    unrelated_operation,
                    unrelated_file,
                    &unrelated_path,
                    &unrelated_hash,
                    1,
                ),
                idempotency_key: None,
            },
        )
        .await
        .unwrap();

    store
        .files()
        .discard_legacy_memory_journals(&context, std::slice::from_ref(&selected))
        .await
        .unwrap();
    let remaining = store.files().list_incomplete(&context).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, unrelated_operation);
    assert!(matches!(
        store
            .files()
            .discard_legacy_memory_journals(&context, std::slice::from_ref(&selected))
            .await,
        Err(mcp_vault_state::StateError::Conflict)
    ));
}

#[tokio::test]
async fn replayed_create_witness_rejects_hash_mismatch() {
    let (
        store,
        context,
        path,
        old_file,
        replacement_file,
        old_operation,
        _replacement_operation,
        _hash,
    ) = replayed_create_fixture().await;
    let mismatch = SupersedeCreateWitness {
        operation_id: old_operation,
        path: path.clone(),
        prior_file_id: old_file,
        replacement_file_id: replacement_file,
        replacement_revision: Revision::new(1),
        physical_hash: "b".repeat(64),
    };
    assert_eq!(
        store
            .files()
            .supersede_replayed_create(&context, &mismatch)
            .await
            .unwrap(),
        SupersedeCreateResult::NotProven
    );
}

#[tokio::test]
async fn replayed_create_witness_rejects_wrong_replacement() {
    let (
        store,
        context,
        path,
        old_file,
        _replacement_file,
        old_operation,
        _replacement_operation,
        hash,
    ) = replayed_create_fixture().await;
    let wrong_replacement = SupersedeCreateWitness {
        operation_id: old_operation,
        path: path.clone(),
        prior_file_id: old_file,
        replacement_file_id: FileId::new(),
        replacement_revision: Revision::new(1),
        physical_hash: hash.clone(),
    };
    assert_eq!(
        store
            .files()
            .supersede_replayed_create(&context, &wrong_replacement)
            .await
            .unwrap(),
        SupersedeCreateResult::NotProven
    );
}

#[tokio::test]
async fn replayed_create_witness_rejects_nonterminal_source_path_claim() {
    let (
        store,
        context,
        path,
        old_file,
        replacement_file,
        old_operation,
        _replacement_operation,
        hash,
    ) = replayed_create_fixture().await;
    let conflicting_operation = OperationId::new();
    let conflicting_file = FileId::new();
    let other_path = VaultPath::parse("_mcp-vault/memory/current/facts/other.md").unwrap();
    store
        .files()
        .prepare_operation(
            &context,
            PrepareOperationInput {
                id: conflicting_operation,
                operation: FileOperation::Move,
                source_path: Some(path.clone()),
                destination_path: Some(other_path),
                prior_file_id: Some(conflicting_file),
                expected_revision: Some(Revision::new(1)),
                prior_hash: Some(hash.clone()),
                proposed_hash: Some(hash.clone()),
                temp_path: Some(VaultPath::parse("_mcp-vault/.tmp/other").unwrap()),
                payload: json!({"operation":"move"}),
                idempotency_key: None,
            },
        )
        .await
        .unwrap();

    let witness = SupersedeCreateWitness {
        operation_id: old_operation,
        path,
        prior_file_id: old_file,
        replacement_file_id: replacement_file,
        replacement_revision: Revision::new(1),
        physical_hash: hash,
    };
    assert_eq!(
        store
            .files()
            .supersede_replayed_create(&context, &witness)
            .await
            .unwrap(),
        SupersedeCreateResult::NotProven
    );
    let incomplete = store.files().list_incomplete(&context).await.unwrap();
    let original = incomplete
        .iter()
        .find(|journal| journal.id == old_operation)
        .unwrap();
    assert_eq!(original.state.as_str(), "file_committed");
}

#[tokio::test]
async fn reviewed_replayed_create_requires_explicit_repair_entrypoint() {
    let (
        store,
        context,
        path,
        old_file,
        replacement_file,
        old_operation,
        _replacement_operation,
        hash,
    ) = replayed_create_fixture().await;
    store
        .files()
        .mark_needs_review(
            &context,
            old_operation,
            "recovery metadata destination already exists",
        )
        .await
        .unwrap();
    let witness = SupersedeCreateWitness {
        operation_id: old_operation,
        path,
        prior_file_id: old_file,
        replacement_file_id: replacement_file,
        replacement_revision: Revision::new(1),
        physical_hash: hash,
    };
    assert_eq!(
        store
            .files()
            .supersede_replayed_create(&context, &witness)
            .await
            .unwrap(),
        SupersedeCreateResult::NotProven
    );
    assert_eq!(
        store
            .files()
            .supersede_reviewed_replayed_create(&context, &witness)
            .await
            .unwrap(),
        SupersedeCreateResult::Applied
    );
}

#[tokio::test]
async fn vault_repository_round_trips_typed_context_and_status() {
    let store = store().await;
    let repository = store.vaults();
    let context = context("work", "/srv/work");

    let inserted = repository
        .insert(&context, "Work Vault", VaultStatus::Active)
        .await
        .unwrap();
    let by_id = repository.find_by_id(context.id()).await.unwrap().unwrap();
    let by_slug = repository
        .find_by_slug(context.slug())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(inserted, by_id);
    assert_eq!(by_id, by_slug);
    assert_eq!(by_id.context().unwrap(), context);

    repository
        .set_status(&context, VaultStatus::Maintenance)
        .await
        .unwrap();
    assert_eq!(
        repository
            .find_by_id(context.id())
            .await
            .unwrap()
            .unwrap()
            .status,
        VaultStatus::Maintenance
    );
}

#[tokio::test]
async fn legacy_default_is_stable_after_a_second_vault_is_registered() {
    let store = store().await;
    let repository = store.vaults();
    let first = context("personal", "/srv/personal");
    insert(&repository, &first).await;

    assert_eq!(
        repository.legacy_default().await.unwrap().unwrap().id,
        first.id()
    );

    let earlier_slug = context("archive", "/srv/archive");
    insert(&repository, &earlier_slug).await;
    assert_eq!(
        repository.legacy_default().await.unwrap().unwrap().id,
        first.id()
    );
}

#[tokio::test]
async fn legacy_default_prefers_the_historical_default_slug() {
    let store = store().await;
    let repository = store.vaults();
    let work = context("work", "/srv/work");
    let default = context("default", "/srv/default");
    insert(&repository, &work).await;
    insert(&repository, &default).await;

    assert_eq!(
        repository.legacy_default().await.unwrap().unwrap().id,
        default.id()
    );
}

#[tokio::test]
async fn legacy_default_fails_closed_when_multiple_vaults_are_ambiguous() {
    let store = store().await;
    let repository = store.vaults();
    insert(&repository, &context("personal", "/srv/personal")).await;
    insert(&repository, &context("work", "/srv/work")).await;

    assert!(repository.legacy_default().await.unwrap().is_none());
}

#[tokio::test]
async fn managed_initialization_job_controls_effective_vault_availability() {
    let store = store().await;
    let context = context("managed", "/srv/managed");
    insert(&store.vaults(), &context).await;
    let vault = store
        .vaults()
        .find_by_id(context.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store.vaults().availability(&vault).await.unwrap(),
        VaultAvailability::Ready
    );

    let job = store
        .jobs()
        .enqueue(
            &context,
            "vault.initialize",
            &format!("vault:{}:initialize", context.id()),
            &json!({}),
            20,
            3,
            0,
        )
        .await
        .unwrap();
    assert_eq!(
        store.vaults().availability(&vault).await.unwrap(),
        VaultAvailability::Initializing
    );
    let claimed = store
        .jobs()
        .claim_batch("availability-test", 1, 60_000, 1)
        .await
        .unwrap();
    assert_eq!(claimed[0].id, job.id);
    store
        .jobs()
        .complete(job.id, "availability-test")
        .await
        .unwrap();
    assert_eq!(
        store.vaults().availability(&vault).await.unwrap(),
        VaultAvailability::Ready
    );
}

#[tokio::test]
async fn journal_summary_includes_review_rows_without_exposing_payload() {
    let store = store().await;
    let context = context("journal-summary", "/srv/journal-summary");
    insert(&store.vaults(), &context).await;

    store
        .files()
        .prepare_operation(
            &context,
            PrepareOperationInput {
                id: OperationId::new(),
                operation: FileOperation::Replace,
                source_path: None,
                destination_path: None,
                prior_file_id: None,
                expected_revision: None,
                prior_hash: None,
                proposed_hash: None,
                temp_path: None,
                payload: json!({"opaque":"must-not-be-returned"}),
                idempotency_key: None,
            },
        )
        .await
        .unwrap();
    let review = store
        .files()
        .prepare_operation(
            &context,
            PrepareOperationInput {
                id: OperationId::new(),
                operation: FileOperation::Delete,
                source_path: None,
                destination_path: None,
                prior_file_id: None,
                expected_revision: None,
                prior_hash: None,
                proposed_hash: None,
                temp_path: None,
                payload: json!({"opaque":"must-not-be-returned"}),
                idempotency_key: None,
            },
        )
        .await
        .unwrap();
    store
        .files()
        .mark_needs_review(&context, review.id, "unsafe detail")
        .await
        .unwrap();

    let summaries = store.files().summarize_incomplete(&context).await.unwrap();
    assert!(summaries.iter().any(|item| {
        item.operation == FileOperation::Replace
            && item.state.as_str() == "prepared"
            && item.count == 1
    }));
    assert!(summaries.iter().any(|item| {
        item.operation == FileOperation::Delete
            && item.state.as_str() == "needs_review"
            && item.count == 1
    }));
}

#[tokio::test]
async fn initialization_task_queue_is_single_writer_under_concurrent_confirmations() {
    let store = store().await;
    let context = context("init-task", "/srv/init-task");
    insert(&store.vaults(), &context).await;
    let first = store.memory_units();
    let second = store.memory_units();
    let (left, right) = tokio::join!(
        first.queue_initialization_task(&context, "task-a", 3, false, "normal"),
        second.queue_initialization_task(&context, "task-b", 3, false, "normal"),
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.task_id, right.task_id);
    assert_eq!(
        store
            .memory_units()
            .initialization_task(&context)
            .await
            .unwrap()
            .unwrap()
            .task_id,
        left.task_id
    );
}

#[tokio::test]
async fn interrupted_initialization_is_marked_failed_and_resumable_on_startup() {
    let store = store().await;
    let context = context("init-interrupted", "/srv/init-interrupted");
    insert(&store.vaults(), &context).await;
    store
        .memory_units()
        .queue_initialization_task(&context, "task-interrupted", 2, false, "normal")
        .await
        .unwrap();
    store
        .memory_units()
        .start_initialization_task(&context, "task-interrupted")
        .await
        .unwrap();
    store
        .memory_units()
        .mark_interrupted_initialization_task(&context)
        .await
        .unwrap();
    let task = store
        .memory_units()
        .initialization_task(&context)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(task.state, "failed");
    assert_eq!(
        task.error_code.as_deref(),
        Some("initialization_interrupted")
    );
    assert!(task.resumable);
}

#[tokio::test]
async fn settings_repository_keeps_two_vaults_isolated() {
    let store = store().await;
    let vaults = store.vaults();
    let settings = store.settings();
    let first = context("first", "/srv/first");
    let second = context("second", "/srv/second");
    insert(&vaults, &first).await;
    insert(&vaults, &second).await;
    let actor = ActorId::new("test-actor").unwrap();

    let first_record = settings
        .set_vault(
            &first,
            "feature.mode",
            &json!({"vault": "first"}),
            WritePrecondition::CreateOnly,
            Some(&actor),
        )
        .await
        .unwrap();
    let second_record = settings
        .set_vault(
            &second,
            "feature.mode",
            &json!({"vault": "second"}),
            WritePrecondition::CreateOnly,
            Some(&actor),
        )
        .await
        .unwrap();

    assert_eq!(first_record.vault_id, Some(first.id()));
    assert_eq!(second_record.vault_id, Some(second.id()));
    assert_eq!(
        settings
            .get_vault(&first, "feature.mode")
            .await
            .unwrap()
            .unwrap()
            .value,
        json!({"vault": "first"})
    );
    assert_eq!(
        settings
            .get_vault(&second, "feature.mode")
            .await
            .unwrap()
            .unwrap()
            .value,
        json!({"vault": "second"})
    );

    let updated = settings
        .set_vault(
            &first,
            "feature.mode",
            &json!({"vault": "first-updated"}),
            WritePrecondition::ExactRevision(first_record.revision),
            Some(&actor),
        )
        .await
        .unwrap();
    assert_eq!(updated.revision, Revision::new(2));

    let stale = settings
        .set_vault(
            &first,
            "feature.mode",
            &json!({"vault": "lost-update"}),
            WritePrecondition::ExactRevision(first_record.revision),
            Some(&actor),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        stale,
        mcp_vault_state::StateError::InvalidDomain(DomainError::RevisionConflict { .. })
    ));
}

#[tokio::test]
async fn recent_revisions_are_vault_scoped_and_bounded() {
    let store = store().await;
    let first = context("first", "/srv/first");
    let second = context("second", "/srv/second");
    insert(&store.vaults(), &first).await;
    insert(&store.vaults(), &second).await;

    assert!(
        store
            .files()
            .list_recent_revisions(&first, 1)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .files()
            .list_recent_revisions(&second, 1)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .files()
            .list_recent_revisions(&first, 0)
            .await
            .is_err()
    );
    assert!(
        store
            .files()
            .list_recent_revisions(&first, 201)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn system_settings_are_global_but_keep_revision_preconditions() {
    let store = store().await;
    let settings = store.settings();

    let first = settings
        .set_system(
            "system.locale",
            &json!("zh-CN"),
            WritePrecondition::CreateOnly,
            None,
        )
        .await
        .unwrap();
    assert_eq!(first.vault_id, None);
    assert_eq!(first.revision, Revision::new(1));

    let error = settings
        .set_system(
            "system.locale",
            &json!("en-US"),
            WritePrecondition::CreateOnly,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        mcp_vault_state::StateError::InvalidDomain(DomainError::PreconditionFailed { .. })
    ));
}

#[tokio::test]
async fn settings_require_a_registered_vault_context() {
    let store = store().await;
    let error = store
        .settings()
        .set_vault(
            &context("unregistered", "/srv/unregistered"),
            "key",
            &json!(true),
            WritePrecondition::CreateOnly,
            None,
        )
        .await
        .unwrap_err();

    assert!(matches!(error, mcp_vault_state::StateError::Database(_)));
}
