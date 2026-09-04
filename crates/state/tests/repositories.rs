use std::path::PathBuf;

use mcp_vault_domain::{
    ActorId, DomainError, MemoryConsolidationId, MemoryId, MemoryRawId, MemoryRetrievalProposalId,
    ModelId, ProviderId, Revision, VaultContext, VaultId, VaultSlug, WritePrecondition,
};
use mcp_vault_state::{
    MemoryBundle, MemoryConsolidationProposalRecord, MemoryFilter, MemoryRecord,
    MemoryRetrievalMetadataRecord, MemoryRetrievalProposalRecord, MemoryStage1OutputRecord,
    StateStore, VaultAvailability, VaultRepository, VaultStatus, memory_search_terms,
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
    let first_pending_fingerprint = repository
        .pending_stage1_fingerprint(&first)
        .await
        .unwrap()
        .unwrap();
    let second_pending_fingerprint = repository
        .pending_stage1_fingerprint(&second)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first_pending_fingerprint, second_pending_fingerprint);
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
    assert_eq!(
        repository.pending_stage1_fingerprint(&first).await.unwrap(),
        None
    );
    assert_eq!(
        repository
            .pending_stage1_fingerprint(&second)
            .await
            .unwrap(),
        Some(second_pending_fingerprint)
    );
    assert!(
        repository
            .get_consolidation_state(&second)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn multilingual_retrieval_metadata_and_fts_are_strictly_vault_scoped() {
    let store = store().await;
    let first = context("first-retrieval", "/srv/first-retrieval");
    let second = context("second-retrieval", "/srv/second-retrieval");
    insert(&store.vaults(), &first).await;
    insert(&store.vaults(), &second).await;
    let repository = store.memory();
    let bundle = |vault_id, id, content: &str, hash: &str| MemoryBundle {
        memory: MemoryRecord {
            id,
            vault_id,
            memory_type: "decision".to_owned(),
            status: "active".to_owned(),
            status_reason: None,
            status_changed_at: None,
            content: content.to_owned(),
            normalized_content: content.to_lowercase(),
            content_hash: hash.to_owned(),
            importance: 0.8,
            confidence: 0.9,
            origin: "explicit_admin".to_owned(),
            revision: Revision::new(1),
            canonical_file_id: None,
            canonical_path: None,
            canonical_revision: None,
            valid_from: None,
            valid_to: None,
            extraction: json!({}),
            created_at: 1,
            updated_at: 1,
            last_recalled_at: None,
            recall_count: 0,
        },
        sources: Vec::new(),
        entities: Vec::new(),
        tags: Vec::new(),
        relations: Vec::new(),
    };
    let first_id = MemoryId::new();
    let second_id = MemoryId::new();
    repository
        .replace_bundle(
            &first,
            &bundle(first.id(), first_id, "项目使用 Rust", "hash-first"),
            None,
        )
        .await
        .unwrap();
    repository
        .replace_bundle(
            &second,
            &bundle(second.id(), second_id, "另一个项目", "hash-second"),
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        repository
            .retrieval_coverage(&first, "profile-v1")
            .await
            .unwrap()
            .pending,
        1
    );
    repository
        .mark_retrieval_backfill_pending(&first, "profile-v1")
        .await
        .unwrap();
    assert_eq!(
        repository
            .retrieval_pending_count(&first, "profile-v1")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        repository
            .retrieval_pending_count(&second, "profile-v1")
            .await
            .unwrap(),
        0
    );
    let aliases = json!([
        {"language": "zh-Hans", "terms": ["项目使用 Rust"]},
        {"language": "en", "terms": ["project uses Rust"]}
    ]);
    let aliases_text = "项目使用 Rust project uses Rust";
    repository
        .upsert_retrieval_metadata(
            &first,
            &MemoryRetrievalMetadataRecord {
                vault_id: first.id(),
                memory_id: first_id,
                content_hash: "hash-first".to_owned(),
                profile_hash: "profile-v1".to_owned(),
                source_language: Some("zh-Hans".to_owned()),
                aliases,
                aliases_text: aliases_text.to_owned(),
                search_terms: memory_search_terms(["项目使用 Rust", aliases_text], 4096),
                status: "ready".to_owned(),
                last_error: None,
                generated_at: Some(2),
                updated_at: 2,
            },
        )
        .await
        .unwrap();

    let first_hits = repository
        .search_fts(&first, "\"project\"", &MemoryFilter::default(), 10)
        .await
        .unwrap();
    let second_hits = repository
        .search_fts(&second, "\"project\"", &MemoryFilter::default(), 10)
        .await
        .unwrap();
    assert_eq!(first_hits.len(), 1);
    assert_eq!(first_hits[0].memory.id, first_id);
    assert!(second_hits.is_empty());
    assert!(
        repository
            .get_retrieval_metadata(&second, first_id)
            .await
            .unwrap()
            .is_none()
    );

    let proposal_id = MemoryRetrievalProposalId::new();
    repository
        .insert_retrieval_proposal(
            &first,
            &MemoryRetrievalProposalRecord {
                id: proposal_id,
                vault_id: first.id(),
                input_hash: "sha256:first-retrieval-input".to_owned(),
                snapshot: json!([]),
                proposal: json!({"version": 1, "items": []}),
                model_id: ModelId::new(),
                provider_id: ProviderId::new(),
                prompt_version: "memory-retrieval-v1".to_owned(),
                status: "prepared".to_owned(),
                applied_count: 0,
                created_at: 3,
                applied_at: None,
            },
        )
        .await
        .unwrap();
    assert!(
        repository
            .get_retrieval_proposal_by_input(&second, "sha256:first-retrieval-input")
            .await
            .unwrap()
            .is_none()
    );

    let first_job = store
        .jobs()
        .enqueue_singleton(
            &first,
            "memory.enrich_retrieval",
            "same-retrieval-job-key",
            &json!({"profile_hash": "profile-v1"}),
            0,
            3,
            4,
        )
        .await
        .unwrap();
    let second_job = store
        .jobs()
        .enqueue_singleton(
            &second,
            "memory.enrich_retrieval",
            "same-retrieval-job-key",
            &json!({"profile_hash": "profile-v1"}),
            0,
            3,
            4,
        )
        .await
        .unwrap();
    assert_ne!(first_job.id, second_job.id);
    assert_eq!(
        store
            .jobs()
            .find_active_by_type(&first, "memory.enrich_retrieval")
            .await
            .unwrap()
            .unwrap()
            .id,
        first_job.id
    );
    assert_eq!(
        store
            .jobs()
            .find_active_by_type(&second, "memory.enrich_retrieval")
            .await
            .unwrap()
            .unwrap()
            .id,
        second_job.id
    );
}
