use std::path::PathBuf;

use mcp_vault_domain::{
    ActorId, DomainError, MemoryConsolidationId, MemoryRawId, ModelId, ProviderId, Revision,
    VaultContext, VaultId, VaultSlug, WritePrecondition,
};
use mcp_vault_state::{
    MemoryConsolidationProposalRecord, MemoryStage1OutputRecord, StateStore, VaultRepository,
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

#[tokio::test]
async fn two_phase_memory_state_is_vault_scoped_and_commits_idempotently() {
    let store = store().await;
    let first = context("first-memory", "/srv/first-memory");
    let second = context("second-memory", "/srv/second-memory");
    insert(&store.vaults(), &first).await;
    insert(&store.vaults(), &second).await;
    let repository = store.memory();

    let first_raw_id = MemoryRawId::new();
    let second_raw_id = MemoryRawId::new();
    let raw_output = |id, vault_id, content: &str| MemoryStage1OutputRecord {
        id,
        vault_id,
        source_type: "explicit_agent".to_owned(),
        source_key: "same-client-key".to_owned(),
        source_file_id: None,
        source_path: None,
        source_revision: None,
        profile_hash: "profile-v1".to_owned(),
        pipeline_version: 1,
        prompt_version: "stage1-v1".to_owned(),
        raw_memory: content.to_owned(),
        source_summary: format!("Summary for {content}"),
        source_slug: Some("explicit-input".to_owned()),
        evidence: json!([]),
        metadata: json!({"memory_type": "decision"}),
        output_hash: format!("hash-{content}"),
        status: "ready".to_owned(),
        generated_at: 10,
        updated_at: 10,
        usage_count: 0,
        last_usage: None,
        selected_for_phase2: false,
        selected_for_phase2_hash: None,
        selected_for_phase2_at: None,
    };
    let first_output = raw_output(first_raw_id, first.id(), "first");
    let second_output = raw_output(second_raw_id, second.id(), "second");
    repository
        .upsert_stage1_output(&first, &first_output)
        .await
        .unwrap();
    repository
        .upsert_stage1_output(&second, &second_output)
        .await
        .unwrap();

    assert_eq!(repository.pending_stage1_count(&first).await.unwrap(), 1);
    assert_eq!(repository.pending_stage1_count(&second).await.unwrap(), 1);
    assert_eq!(
        repository
            .get_stage1_output(&first, "explicit_agent", "same-client-key")
            .await
            .unwrap()
            .unwrap()
            .raw_memory,
        "first"
    );
    let cross_vault = repository
        .upsert_stage1_output(&first, &second_output)
        .await
        .unwrap_err();
    assert!(matches!(
        cross_vault,
        mcp_vault_state::StateError::InvalidInput(_)
    ));

    let proposal_id = MemoryConsolidationId::new();
    repository
        .insert_consolidation_proposal(
            &first,
            &MemoryConsolidationProposalRecord {
                id: proposal_id,
                vault_id: first.id(),
                input_hash: "input-first-v1".to_owned(),
                proposal: json!({"memory_summary": "first summary"}),
                model_id: ModelId::new(),
                provider_id: ProviderId::new(),
                prompt_version: "consolidation-v1".to_owned(),
                status: "prepared".to_owned(),
                created_at: 20,
                applied_at: None,
            },
        )
        .await
        .unwrap();
    let selected = vec![(first_raw_id, first_output.output_hash.clone())];
    let committed = repository
        .commit_consolidation(
            &first,
            proposal_id,
            "input-first-v1",
            "first summary",
            &selected,
        )
        .await
        .unwrap();
    let repeated = repository
        .commit_consolidation(
            &first,
            proposal_id,
            "input-first-v1",
            "first summary",
            &selected,
        )
        .await
        .unwrap();

    assert_eq!(committed.generation, 1);
    assert_eq!(repeated.generation, 1);
    assert_eq!(repository.pending_stage1_count(&first).await.unwrap(), 0);
    assert_eq!(repository.pending_stage1_count(&second).await.unwrap(), 1);
    assert!(
        repository
            .get_consolidation_state(&second)
            .await
            .unwrap()
            .is_none()
    );
}
