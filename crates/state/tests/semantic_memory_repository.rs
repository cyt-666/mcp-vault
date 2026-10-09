use std::{path::PathBuf, str::FromStr};

use mcp_vault_domain::{
    Actor, ActorType, CardItemId, CardRevisionId, EvidenceRefId, ExtractionSetId, FileId,
    MemoryCardId, ObservationId, OperationId, Revision, SourcePlane, VaultContext, VaultId,
    VaultPath, VaultSlug,
};
use mcp_vault_state::{
    CommitMutationInput, EntryType, FileOperation, NoopCommitHook, PrepareOperationInput,
    SemanticCardItemInput, SemanticCardRevisionInput, SemanticEvidenceInput,
    SemanticExtractionBatchSpec, SemanticObservationInput, SemanticSpanInput, StateStore,
    VaultStatus,
};
use serde_json::json;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

fn context(slug: &str) -> VaultContext {
    VaultContext::new(
        VaultId::new(),
        VaultSlug::new(slug).unwrap(),
        PathBuf::from(format!("/tmp/mcp-vault-semantic-{slug}")),
        Revision::ZERO,
    )
    .unwrap()
}

#[tokio::test]
async fn extraction_batches_are_revision_scoped_monotonic_and_fail_closed_on_resume() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let first = context("batch-first");
    let second = context("batch-second");
    for (ctx, name) in [(&first, "batch-first"), (&second, "batch-second")] {
        state
            .vaults()
            .insert(ctx, name, VaultStatus::Active)
            .await
            .unwrap();
    }
    let file_id = FileId::new();
    let path = VaultPath::parse("notes/source.md").unwrap();
    register_file(
        &state,
        &first,
        file_id,
        &path,
        Revision::new(1),
        "batch-content",
    )
    .await;
    let (source, revision, _) = state
        .semantic_memory()
        .upsert_source_revision(&first, file_id, &path, Revision::new(1), "batch-content", 0)
        .await
        .unwrap();
    let extraction_id = ExtractionSetId::new();
    state
        .semantic_memory()
        .start_extraction(
            &first,
            extraction_id,
            source.source_id,
            revision.source_revision_id,
            "batch-content",
            "semantic-memory-a80-v1",
            "batch-idempotency",
            "batch-request",
        )
        .await
        .unwrap();
    let spec = SemanticExtractionBatchSpec {
        batch_index: 0,
        batch_count: 1,
        source_revision_id: revision.source_revision_id,
        batch_input_hash: "a".repeat(64),
        batch_catalog_hash: "b".repeat(64),
        prompt_id: "semantic-cards-tracked-adr-m6-v13".to_owned(),
        schema_id: "semantic-cards-m6-json-v9".to_owned(),
        provider_fingerprint: "provider-model-binding".to_owned(),
    };
    let batches = state
        .semantic_memory()
        .prepare_extraction_batches(&first, extraction_id, std::slice::from_ref(&spec))
        .await
        .unwrap();
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].state, "ready");
    assert!(
        state
            .semantic_memory()
            .list_extraction_batches(&second, extraction_id)
            .await
            .unwrap()
            .is_empty()
    );
    let attempt = state
        .semantic_memory()
        .reserve_extraction_batch_attempt(&first, extraction_id, 0, &spec.batch_input_hash)
        .await
        .unwrap();
    assert_eq!(attempt.state, "dispatching");
    assert_eq!(attempt.attempt_count, 1);
    state
        .semantic_memory()
        .fail_extraction_batch_attempt(
            &first,
            extraction_id,
            0,
            &spec.batch_input_hash,
            "invalid_core_proposal",
        )
        .await
        .unwrap();
    let attempt = state
        .semantic_memory()
        .reserve_extraction_batch_regen_attempt(&first, extraction_id, 0, &spec.batch_input_hash)
        .await
        .unwrap();
    assert_eq!(attempt.attempt_count, 2);
    state
        .semantic_memory()
        .fail_extraction_batch_attempt(
            &first,
            extraction_id,
            0,
            &spec.batch_input_hash,
            "invalid_core_proposal",
        )
        .await
        .unwrap();
    assert!(
        state
            .semantic_memory()
            .reserve_extraction_batch_attempt(&first, extraction_id, 0, &spec.batch_input_hash,)
            .await
            .is_err()
    );
    assert!(
        state
            .semantic_memory()
            .reserve_extraction_batch_regen_attempt(
                &first,
                extraction_id,
                0,
                &spec.batch_input_hash,
            )
            .await
            .is_err()
    );

    // The source-wide regen token can be consumed only once even when the
    // extraction has several independent batches. A validated first batch
    // stays resumable while a later batch is repaired.
    let regen_id = ExtractionSetId::new();
    state
        .semantic_memory()
        .start_extraction(
            &first,
            regen_id,
            source.source_id,
            revision.source_revision_id,
            "batch-content",
            "semantic-memory-a80-v1",
            "batch-idempotency-regen",
            "batch-request-regen",
        )
        .await
        .unwrap();
    let regen_specs = (0..3)
        .map(|batch_index| SemanticExtractionBatchSpec {
            batch_index,
            batch_count: 3,
            source_revision_id: revision.source_revision_id,
            batch_input_hash: format!("{}", (b'd' + batch_index as u8) as char).repeat(64),
            batch_catalog_hash: "f".repeat(64),
            prompt_id: "semantic-cards-tracked-adr-m6-v13".to_owned(),
            schema_id: "semantic-cards-m6-json-v9".to_owned(),
            provider_fingerprint: "provider-model-binding".to_owned(),
        })
        .collect::<Vec<_>>();
    state
        .semantic_memory()
        .prepare_extraction_batches(&first, regen_id, &regen_specs)
        .await
        .unwrap();
    let first_batch = &regen_specs[0];
    state
        .semantic_memory()
        .reserve_extraction_batch_attempt(&first, regen_id, 0, &first_batch.batch_input_hash)
        .await
        .unwrap();
    state
        .semantic_memory()
        .store_validated_extraction_batch(
            &first,
            regen_id,
            0,
            &first_batch.batch_input_hash,
            "[]",
            &json!({}),
        )
        .await
        .unwrap();
    for batch_index in [1_u32, 2_u32] {
        let spec = &regen_specs[batch_index as usize];
        state
            .semantic_memory()
            .reserve_extraction_batch_attempt(&first, regen_id, batch_index, &spec.batch_input_hash)
            .await
            .unwrap();
        state
            .semantic_memory()
            .fail_extraction_batch_attempt(
                &first,
                regen_id,
                batch_index,
                &spec.batch_input_hash,
                "invalid_core_proposal",
            )
            .await
            .unwrap();
    }
    let repaired = state
        .semantic_memory()
        .reserve_extraction_batch_regen_attempt(
            &first,
            regen_id,
            1,
            &regen_specs[1].batch_input_hash,
        )
        .await
        .unwrap();
    assert_eq!(repaired.state, "dispatching");
    assert_eq!(repaired.attempt_count, 2);
    assert!(
        state
            .semantic_memory()
            .reserve_extraction_batch_regen_attempt(
                &first,
                regen_id,
                2,
                &regen_specs[2].batch_input_hash,
            )
            .await
            .is_err(),
        "a second batch must not receive another source-wide regen token"
    );
    let resumed = state
        .semantic_memory()
        .list_extraction_batches(&first, regen_id)
        .await
        .unwrap();
    assert_eq!(resumed[0].state, "validated");
    assert_eq!(resumed[0].attempt_count, 1);

    // A process crash after dispatch reservation is ambiguous: listing it
    // must not replay the provider request or refund the monotonic attempt.
    let interrupted_id = ExtractionSetId::new();
    state
        .semantic_memory()
        .start_extraction(
            &first,
            interrupted_id,
            source.source_id,
            revision.source_revision_id,
            "batch-content",
            "semantic-memory-a80-v1",
            "batch-idempotency-2",
            "batch-request-2",
        )
        .await
        .unwrap();
    let interrupted = state
        .semantic_memory()
        .prepare_extraction_batches(&first, interrupted_id, std::slice::from_ref(&spec))
        .await
        .unwrap();
    assert_eq!(interrupted[0].state, "ready");
    state
        .semantic_memory()
        .reserve_extraction_batch_attempt(&first, interrupted_id, 0, &spec.batch_input_hash)
        .await
        .unwrap();
    assert!(
        state
            .semantic_memory()
            .reserve_extraction_batch_attempt(&first, interrupted_id, 0, &spec.batch_input_hash,)
            .await
            .is_err()
    );
    let rows = state
        .semantic_memory()
        .list_extraction_batches(&first, interrupted_id)
        .await
        .unwrap();
    assert_eq!(rows[0].state, "uncertain");
    assert_eq!(rows[0].attempt_count, 1);

    let mut drifted = spec;
    drifted.batch_input_hash = "c".repeat(64);
    assert!(
        state
            .semantic_memory()
            .prepare_extraction_batches(&first, extraction_id, &[drifted],)
            .await
            .is_err()
    );
    assert!(
        state
            .semantic_memory()
            .list_extraction_batches(&second, interrupted_id)
            .await
            .unwrap()
            .is_empty()
    );
}

async fn register_file(
    state: &StateStore,
    context: &VaultContext,
    file_id: FileId,
    path: &VaultPath,
    revision: Revision,
    content_hash: &str,
) {
    let operation_id = OperationId::new();
    state
        .files()
        .prepare_operation(
            context,
            PrepareOperationInput {
                id: operation_id,
                operation: FileOperation::Create,
                source_path: Some(path.clone()),
                destination_path: Some(path.clone()),
                prior_file_id: Some(file_id),
                expected_revision: None,
                prior_hash: None,
                proposed_hash: Some(content_hash.to_owned()),
                temp_path: Some(VaultPath::parse("_mcp-vault/.tmp/semantic-test").unwrap()),
                payload: json!({"test": "semantic-memory"}),
                idempotency_key: None,
            },
        )
        .await
        .unwrap();
    state
        .files()
        .mark_file_committed(context, operation_id, Some(content_hash))
        .await
        .unwrap();
    state
        .files()
        .commit_mutation(
            context,
            CommitMutationInput {
                operation_id,
                file_id,
                entry_type: EntryType::File,
                path: path.clone(),
                path_before: None,
                path_after: Some(path.clone()),
                expected_revision: None,
                require_absent: true,
                tombstone_archive_path: None,
                content_hash: Some(content_hash.to_owned()),
                history_blob_hash: None,
                size: 1,
                modified_at: 1,
                filesystem_identity: None,
                deleted_at: None,
                operation: FileOperation::Create,
                actor: Actor::new(ActorType::System, None),
                source_plane: SourcePlane::System,
                idempotency_key: None,
                audit_action: "test.semantic_source_create".to_owned(),
                audit_metadata: json!({}),
                request_id: None,
                outbox_events: Vec::new(),
            },
            &NoopCommitHook,
        )
        .await
        .unwrap();
    assert_eq!(
        state
            .files()
            .get_by_id(context, file_id)
            .await
            .unwrap()
            .unwrap()
            .current_revision,
        revision
    );
}

fn observation(
    observation_id: ObservationId,
    evidence_id: EvidenceRefId,
    local_key: &str,
) -> SemanticObservationInput {
    SemanticObservationInput {
        id: observation_id,
        local_key: local_key.to_owned(),
        kind: "decision".to_owned(),
        statement: "Keep the source revision boundary.".to_owned(),
        scope: "project".to_owned(),
        assertion_status: "source_asserted".to_owned(),
        source_time_scope: json!({}),
        conditions: Vec::new(),
        exceptions: Vec::new(),
        ordered_steps: Vec::new(),
        result: None,
        uncertainty: None,
        admission_reason: "boundary".to_owned(),
        value_for_future_work: "preserve evidence".to_owned(),
        evidence: SemanticEvidenceInput {
            id: evidence_id,
            body_spans: vec![SemanticSpanInput {
                start_byte: 0,
                end_byte: 1,
                content_hash: "span".to_owned(),
            }],
            context_spans: Vec::new(),
        },
    }
}

fn card(
    card_id: MemoryCardId,
    revision_id: CardRevisionId,
    topic_key: &str,
    expected_card_revision_id: Option<CardRevisionId>,
    item: SemanticCardItemInput,
) -> SemanticCardRevisionInput {
    SemanticCardRevisionInput {
        card_id,
        card_revision_id: revision_id,
        expected_card_revision_id,
        revision_number: 1,
        topic_key: topic_key.to_owned(),
        title: "Revision boundary".to_owned(),
        kind: "decision".to_owned(),
        scope_ref: "project".to_owned(),
        assertion_status: "source_asserted".to_owned(),
        temporal_scope: json!({}),
        composition_profile_id: "semantic-memory-m1-v1".to_owned(),
        canonical_path: VaultPath::parse("_mcp-vault/semantic-memory/cards/test.md").unwrap(),
        canonical_markdown_hash: "card-hash".to_owned(),
        canonical_bytes: b"card".to_vec(),
        items: vec![item],
    }
}

#[tokio::test]
async fn semantic_namespace_migrates_and_is_vault_scoped() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let first = context("first");
    let second = context("second");
    state
        .vaults()
        .insert(&first, "first", VaultStatus::Active)
        .await
        .unwrap();
    state
        .vaults()
        .insert(&second, "second", VaultStatus::Active)
        .await
        .unwrap();
    let report = state.integrity_check().await.unwrap();
    assert!(report.integrity_ok);
    assert_eq!(report.migration_version, 44);
    assert!(
        state
            .semantic_memory()
            .pending_extractions(&first)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        state
            .semantic_memory()
            .pending_extractions(&second)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn prepare_publication_rejects_card_profile_mismatch_atomically() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context("profile-fence");
    state
        .vaults()
        .insert(&context, "profile-fence", VaultStatus::Active)
        .await
        .unwrap();
    let file_id = FileId::new();
    let path = VaultPath::parse("notes/profile.md").unwrap();
    register_file(
        &state,
        &context,
        file_id,
        &path,
        Revision::new(1),
        "profile-hash",
    )
    .await;
    let (source, revision, _) = state
        .semantic_memory()
        .upsert_source_revision(
            &context,
            file_id,
            &path,
            Revision::new(1),
            "profile-hash",
            0,
        )
        .await
        .unwrap();
    let extraction = ExtractionSetId::new();
    state
        .semantic_memory()
        .start_extraction(
            &context,
            extraction,
            source.source_id,
            revision.source_revision_id,
            "profile-hash",
            "semantic-memory-m1-v1",
            "profile-fence",
            "profile-fence-request",
        )
        .await
        .unwrap();
    let observation_id = ObservationId::new();
    let evidence_id = EvidenceRefId::new();
    let mut invalid = card(
        MemoryCardId::new(),
        CardRevisionId::new(),
        "profile-fence",
        None,
        SemanticCardItemInput {
            id: CardItemId::new(),
            observation_id,
            kind: "core_assertion".to_owned(),
            ordinal: 0,
            content: "Keep the source revision boundary.".to_owned(),
            evidence_ref_ids: vec![evidence_id],
        },
    );
    invalid.composition_profile_id = "wrong-profile".to_owned();
    assert!(
        state
            .semantic_memory()
            .prepare_publication(
                &context,
                extraction,
                &[observation(observation_id, evidence_id, "profile-fence")],
                &[invalid],
            )
            .await
            .is_err()
    );

    let valid = card(
        MemoryCardId::new(),
        CardRevisionId::new(),
        "profile-fence-valid",
        None,
        SemanticCardItemInput {
            id: CardItemId::new(),
            observation_id: ObservationId::new(),
            kind: "core_assertion".to_owned(),
            ordinal: 0,
            content: "Keep the source revision boundary.".to_owned(),
            evidence_ref_ids: vec![EvidenceRefId::new()],
        },
    );
    let valid_observation_id = valid.items[0].observation_id;
    let valid_evidence_id = valid.items[0].evidence_ref_ids[0];
    let snapshots = state
        .semantic_memory()
        .prepare_publication(
            &context,
            extraction,
            &[observation(
                valid_observation_id,
                valid_evidence_id,
                "profile-fence-valid",
            )],
            &[valid],
        )
        .await
        .unwrap();
    assert_eq!(snapshots.len(), 1);
}

#[tokio::test]
async fn rules_change_blocks_old_success_empty_before_card_invalidation() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context("rules-empty");
    state
        .vaults()
        .insert(&context, "rules-empty", VaultStatus::Active)
        .await
        .unwrap();
    let file_id = FileId::new();
    let path = VaultPath::parse("notes/rules-empty.md").unwrap();
    register_file(
        &state,
        &context,
        file_id,
        &path,
        Revision::new(1),
        "rules-empty-hash",
    )
    .await;
    let (source, revision, _) = state
        .semantic_memory()
        .upsert_source_revision(
            &context,
            file_id,
            &path,
            Revision::new(1),
            "rules-empty-hash",
            0,
        )
        .await
        .unwrap();
    let extraction_id = ExtractionSetId::new();
    state
        .semantic_memory()
        .start_extraction(
            &context,
            extraction_id,
            source.source_id,
            revision.source_revision_id,
            &source.content_hash,
            "semantic-memory-m1-v1",
            "rules-empty-extraction",
            "rules-empty-request",
        )
        .await
        .unwrap();
    state
        .semantic_rules()
        .apply_correction(
            &context,
            "source_extraction",
            "source",
            1,
            &format!(
                "{}:{}:semantic-memory-m1-v1",
                source.source_id, revision.source_revision_id
            ),
            &json!({"replace":"blocked","remove":"candidate"}),
            None,
            None,
            None,
            None,
            None,
            None,
            "test-actor",
            "rules-empty-correction",
        )
        .await
        .unwrap();
    assert!(
        state
            .semantic_memory()
            .finish_empty_or_nonpublishable(&context, extraction_id, "success_empty", None,)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn invalidating_unknown_source_does_not_cross_vaults() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let first = context("first");
    let second = context("second");
    state
        .vaults()
        .insert(&first, "first", VaultStatus::Active)
        .await
        .unwrap();
    state
        .vaults()
        .insert(&second, "second", VaultStatus::Active)
        .await
        .unwrap();
    let file_id = mcp_vault_domain::FileId::new();
    assert!(
        !state
            .semantic_memory()
            .invalidate_source(&first, file_id, "permission_revoked", Some(2))
            .await
            .unwrap()
    );
    assert!(
        state
            .semantic_memory()
            .get_source_by_file(&second, file_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn running_extraction_cancellation_is_terminal_and_vault_scoped() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("semantic-cancel.sqlite3");
    let url = format!("sqlite://{}", database.display());
    let state = StateStore::connect_and_migrate(&url).await.unwrap();
    let first = context("cancel-first");
    let second = context("cancel-second");
    state
        .vaults()
        .insert(&first, "cancel-first", VaultStatus::Active)
        .await
        .unwrap();
    state
        .vaults()
        .insert(&second, "cancel-second", VaultStatus::Active)
        .await
        .unwrap();
    let file_id = FileId::new();
    let path = VaultPath::parse("notes/cancel.md").unwrap();
    register_file(
        &state,
        &first,
        file_id,
        &path,
        Revision::new(1),
        "cancel-hash",
    )
    .await;
    let (source, revision, _) = state
        .semantic_memory()
        .upsert_source_revision(&first, file_id, &path, Revision::new(1), "cancel-hash", 0)
        .await
        .unwrap();
    let extraction = ExtractionSetId::new();
    state
        .semantic_memory()
        .start_extraction(
            &first,
            extraction,
            source.source_id,
            revision.source_revision_id,
            "cancel-hash",
            "semantic-memory-m1-v1",
            "cancel-once",
            "cancel-request",
        )
        .await
        .unwrap();
    state
        .semantic_memory()
        .cancel_extraction(&first, extraction, "semantic_extraction_cancelled")
        .await
        .unwrap();

    for (id, key) in [
        (ExtractionSetId::new(), "prepared-cancel"),
        (ExtractionSetId::new(), "written-cancel"),
    ] {
        state
            .semantic_memory()
            .start_extraction(
                &first,
                id,
                source.source_id,
                revision.source_revision_id,
                "cancel-hash",
                "semantic-memory-m1-v1",
                key,
                key,
            )
            .await
            .unwrap();
        let observation_id = ObservationId::new();
        let evidence_id = EvidenceRefId::new();
        let snapshots = state
            .semantic_memory()
            .prepare_publication(
                &first,
                id,
                &[observation(observation_id, evidence_id, key)],
                &[card(
                    MemoryCardId::new(),
                    CardRevisionId::new(),
                    key,
                    None,
                    SemanticCardItemInput {
                        id: CardItemId::new(),
                        observation_id,
                        kind: "core_assertion".to_owned(),
                        ordinal: 0,
                        content: "Keep the source revision boundary.".to_owned(),
                        evidence_ref_ids: vec![evidence_id],
                    },
                )],
            )
            .await
            .unwrap();
        if key == "written-cancel" {
            state
                .semantic_memory()
                .mark_snapshot_written(&first, snapshots[0].id, file_id, Revision::new(1))
                .await
                .unwrap();
        }
        assert!(
            state
                .semantic_memory()
                .cancel_extraction(&first, id, "semantic_extraction_cancelled")
                .await
                .is_err()
        );
        assert_eq!(
            state
                .semantic_memory()
                .get_extraction(&first, id)
                .await
                .unwrap()
                .unwrap()
                .state,
            "prepared"
        );
    }
    state.close().await;

    let reopened = StateStore::connect_and_migrate(&url).await.unwrap();
    assert_eq!(
        reopened
            .semantic_memory()
            .get_extraction(&first, extraction)
            .await
            .unwrap()
            .unwrap()
            .state,
        "cancelled"
    );
    assert!(
        !reopened
            .semantic_memory()
            .pending_extractions(&first)
            .await
            .unwrap()
            .contains(&extraction)
    );
    assert!(
        reopened
            .semantic_memory()
            .get_extraction(&second, extraction)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        reopened
            .semantic_memory()
            .cancel_extraction(&first, extraction, "semantic_extraction_cancelled")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn semantic_schema_guard_only_skips_pre_semantic_databases() {
    async fn check(
        version: Option<i64>,
        with_sources: bool,
    ) -> Result<bool, mcp_vault_state::StateError> {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("partial.sqlite");
        let url = format!("sqlite://{}", database.display());
        let mut connection = SqliteConnection::connect_with(
            &SqliteConnectOptions::from_str(&url)
                .unwrap()
                .create_if_missing(true),
        )
        .await
        .unwrap();
        if let Some(version) = version {
            sqlx::query(
                "CREATE TABLE _sqlx_migrations (version INTEGER NOT NULL, success INTEGER NOT NULL)",
            )
            .execute(&mut connection)
            .await
            .unwrap();
            sqlx::query("INSERT INTO _sqlx_migrations(version,success) VALUES(?,1)")
                .bind(version)
                .execute(&mut connection)
                .await
                .unwrap();
        }
        if with_sources {
            sqlx::query("CREATE TABLE semantic_sources (source_id TEXT)")
                .execute(&mut connection)
                .await
                .unwrap();
        }
        connection.close().await.unwrap();

        let state = StateStore::connect(&url).await.unwrap();
        let context = context("schema-guard");
        state
            .semantic_memory()
            .invalidate_source(&context, FileId::new(), "source_changed", None)
            .await
    }

    assert!(!check(None, false).await.unwrap());
    assert!(!check(Some(27), false).await.unwrap());
    assert!(matches!(
        check(Some(43), true).await,
        Err(mcp_vault_state::StateError::IntegrityFailure)
    ));
}

#[tokio::test]
async fn policy_invalidation_and_new_file_id_are_vault_and_identity_scoped() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let first = context("policy-first");
    let second = context("policy-second");
    state
        .vaults()
        .insert(&first, "policy-first", VaultStatus::Active)
        .await
        .unwrap();
    state
        .vaults()
        .insert(&second, "policy-second", VaultStatus::Active)
        .await
        .unwrap();
    let path = VaultPath::parse("notes/policy.md").unwrap();
    let first_file = FileId::new();
    let second_file = FileId::new();
    for (vault, file_id, hash) in [
        (&first, first_file, "first-policy"),
        (&second, second_file, "second-policy"),
    ] {
        register_file(&state, vault, file_id, &path, Revision::new(1), hash).await;
        let (source, revision, _) = state
            .semantic_memory()
            .upsert_source_revision(vault, file_id, &path, Revision::new(1), hash, 0)
            .await
            .unwrap();
        let extraction_id = ExtractionSetId::new();
        state
            .semantic_memory()
            .start_extraction(
                vault,
                extraction_id,
                source.source_id,
                revision.source_revision_id,
                hash,
                "semantic-memory-m1-v1",
                &format!("policy-{file_id}"),
                &format!("policy-request-{file_id}"),
            )
            .await
            .unwrap();
        state
            .semantic_memory()
            .finish_empty_or_nonpublishable(vault, extraction_id, "success_empty", None)
            .await
            .unwrap();
    }
    state
        .settings()
        .set_vault(
            &first,
            "memory.units.policy",
            &json!({"enabled":true,"request_timeout_seconds":301}),
            mcp_vault_domain::WritePrecondition::Unconditional,
            None,
        )
        .await
        .unwrap();
    assert!(
        !state
            .semantic_memory()
            .get_source_by_file(&first, first_file)
            .await
            .unwrap()
            .unwrap()
            .eligible
    );
    assert!(
        state
            .semantic_memory()
            .get_source_by_file(&second, second_file)
            .await
            .unwrap()
            .unwrap()
            .eligible
    );

    // Tombstone the first File ID, then create the same path with a new ID.
    let delete_operation = OperationId::new();
    state
        .files()
        .prepare_operation(
            &first,
            PrepareOperationInput {
                id: delete_operation,
                operation: FileOperation::Delete,
                source_path: Some(path.clone()),
                destination_path: Some(path.clone()),
                prior_file_id: Some(first_file),
                expected_revision: Some(Revision::new(1)),
                prior_hash: Some("first-policy".to_owned()),
                proposed_hash: None,
                temp_path: None,
                payload: json!({"test":"delete"}),
                idempotency_key: None,
            },
        )
        .await
        .unwrap();
    state
        .files()
        .mark_file_committed(&first, delete_operation, None)
        .await
        .unwrap();
    state
        .files()
        .commit_mutation(
            &first,
            CommitMutationInput {
                operation_id: delete_operation,
                file_id: first_file,
                entry_type: EntryType::File,
                path: path.clone(),
                path_before: Some(path.clone()),
                path_after: Some(path.clone()),
                expected_revision: Some(Revision::new(1)),
                require_absent: false,
                tombstone_archive_path: None,
                content_hash: None,
                history_blob_hash: None,
                size: 0,
                modified_at: 2,
                filesystem_identity: None,
                deleted_at: Some(2),
                operation: FileOperation::Delete,
                actor: Actor::new(ActorType::System, None),
                source_plane: SourcePlane::System,
                idempotency_key: None,
                audit_action: "test.semantic_delete".to_owned(),
                audit_metadata: json!({}),
                request_id: None,
                outbox_events: Vec::new(),
            },
            &NoopCommitHook,
        )
        .await
        .unwrap();
    let new_file = FileId::new();
    let create_operation = OperationId::new();
    state
        .files()
        .prepare_operation(
            &first,
            PrepareOperationInput {
                id: create_operation,
                operation: FileOperation::Create,
                source_path: Some(path.clone()),
                destination_path: Some(path.clone()),
                prior_file_id: Some(new_file),
                expected_revision: None,
                prior_hash: None,
                proposed_hash: Some("new-policy".to_owned()),
                temp_path: None,
                payload: json!({"test":"create"}),
                idempotency_key: None,
            },
        )
        .await
        .unwrap();
    state
        .files()
        .mark_file_committed(&first, create_operation, Some("new-policy"))
        .await
        .unwrap();
    state
        .files()
        .commit_mutation(
            &first,
            CommitMutationInput {
                operation_id: create_operation,
                file_id: new_file,
                entry_type: EntryType::File,
                path,
                path_before: None,
                path_after: Some(VaultPath::parse("notes/policy.md").unwrap()),
                expected_revision: None,
                require_absent: true,
                tombstone_archive_path: Some(
                    VaultPath::parse(&format!("_mcp-vault/tombstones/{first_file}")).unwrap(),
                ),
                content_hash: Some("new-policy".to_owned()),
                history_blob_hash: None,
                size: 1,
                modified_at: 3,
                filesystem_identity: None,
                deleted_at: None,
                operation: FileOperation::Create,
                actor: Actor::new(ActorType::System, None),
                source_plane: SourcePlane::System,
                idempotency_key: None,
                audit_action: "test.semantic_recreate".to_owned(),
                audit_metadata: json!({}),
                request_id: None,
                outbox_events: Vec::new(),
            },
            &NoopCommitHook,
        )
        .await
        .unwrap();
    assert!(
        state
            .semantic_memory()
            .get_source_by_file(&first, new_file)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !state
            .semantic_memory()
            .get_source_by_file(&first, first_file)
            .await
            .unwrap()
            .unwrap()
            .eligible
    );
}

#[tokio::test]
async fn repository_rejects_cross_revision_card_support_atomically() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context("revision-fence");
    state
        .vaults()
        .insert(&context, "revision-fence", VaultStatus::Active)
        .await
        .unwrap();
    let file_id = FileId::new();
    let path = VaultPath::parse("notes/source.md").unwrap();
    register_file(
        &state,
        &context,
        file_id,
        &path,
        Revision::new(1),
        "hash-one",
    )
    .await;

    let repository = state.semantic_memory();
    let (source, first_revision, _) = repository
        .upsert_source_revision(&context, file_id, &path, Revision::new(1), "hash-one", 0)
        .await
        .unwrap();
    let first_extraction_id = ExtractionSetId::new();
    repository
        .start_extraction(
            &context,
            first_extraction_id,
            source.source_id,
            first_revision.source_revision_id,
            "hash-one",
            "semantic-memory-m1-v1",
            "first-revision",
            "first-revision-request",
        )
        .await
        .unwrap();
    let old_observation_id = ObservationId::new();
    let old_evidence_id = EvidenceRefId::new();
    let old_observation = observation(old_observation_id, old_evidence_id, "old");
    repository
        .prepare_publication(
            &context,
            first_extraction_id,
            std::slice::from_ref(&old_observation),
            &[card(
                MemoryCardId::new(),
                CardRevisionId::new(),
                "old-card",
                None,
                SemanticCardItemInput {
                    id: CardItemId::new(),
                    observation_id: old_observation_id,
                    kind: "core_assertion".to_owned(),
                    ordinal: 0,
                    content: old_observation.statement.clone(),
                    evidence_ref_ids: vec![old_evidence_id],
                },
            )],
        )
        .await
        .unwrap();

    let (_, second_revision, changed) = repository
        .upsert_source_revision(&context, file_id, &path, Revision::new(1), "hash-two", 0)
        .await
        .unwrap();
    assert!(changed);
    let second_extraction_id = ExtractionSetId::new();
    repository
        .start_extraction(
            &context,
            second_extraction_id,
            source.source_id,
            second_revision.source_revision_id,
            "hash-two",
            "semantic-memory-m1-v1",
            "second-revision",
            "second-revision-request",
        )
        .await
        .unwrap();

    let current_observation_id = ObservationId::new();
    let current_evidence_id = EvidenceRefId::new();
    let current_observation = observation(current_observation_id, current_evidence_id, "current");
    let mixed_item = SemanticCardItemInput {
        id: CardItemId::new(),
        // This observation belongs to the old SourceRevision.  The caller is
        // intentionally bypassing SemanticMemoryService's proposal checks.
        observation_id: old_observation_id,
        kind: "core_assertion".to_owned(),
        ordinal: 0,
        content: "cross-revision item".to_owned(),
        evidence_ref_ids: vec![old_evidence_id],
    };
    let result = repository
        .prepare_publication(
            &context,
            second_extraction_id,
            std::slice::from_ref(&current_observation),
            &[card(
                MemoryCardId::new(),
                CardRevisionId::new(),
                "mixed-card",
                None,
                mixed_item,
            )],
        )
        .await;
    assert!(
        result.is_err(),
        "cross-revision support must be rejected by State"
    );

    // The transaction must roll back every inserted evidence, observation,
    // card and snapshot; a failed projection cannot leave a half-published set.
    assert_eq!(
        repository
            .get_extraction(&context, second_extraction_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "running"
    );
    assert!(
        repository
            .list_observations(&context, second_extraction_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repository
            .pending_snapshots(&context, second_extraction_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repository
            .card_head(&context, source.source_id, "mixed-card")
            .await
            .unwrap()
            .is_none()
    );

    // A current-revision observation paired with an older evidence reference
    // must fail at the evidence fence as well, not merely at the observation
    // fence above.
    let evidence_extraction_id = ExtractionSetId::new();
    repository
        .start_extraction(
            &context,
            evidence_extraction_id,
            source.source_id,
            second_revision.source_revision_id,
            "hash-two",
            "semantic-memory-m1-v1",
            "evidence-revision",
            "evidence-revision-request",
        )
        .await
        .unwrap();
    let evidence_observation_id = ObservationId::new();
    let evidence_observation = observation(
        evidence_observation_id,
        EvidenceRefId::new(),
        "evidence-current",
    );
    let evidence_mismatch = repository
        .prepare_publication(
            &context,
            evidence_extraction_id,
            std::slice::from_ref(&evidence_observation),
            &[card(
                MemoryCardId::new(),
                CardRevisionId::new(),
                "mixed-evidence-card",
                None,
                SemanticCardItemInput {
                    id: CardItemId::new(),
                    observation_id: evidence_observation_id,
                    kind: "core_assertion".to_owned(),
                    ordinal: 0,
                    content: "cross-revision evidence".to_owned(),
                    evidence_ref_ids: vec![old_evidence_id],
                },
            )],
        )
        .await;
    assert!(evidence_mismatch.is_err());
    assert!(
        repository
            .list_observations(&context, evidence_extraction_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repository
            .pending_snapshots(&context, evidence_extraction_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn stale_written_snapshot_cannot_apply_after_source_generation_changes() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context("stale-snapshot");
    state
        .vaults()
        .insert(&context, "stale-snapshot", VaultStatus::Active)
        .await
        .unwrap();
    let file_id = FileId::new();
    let path = VaultPath::parse("notes/source.md").unwrap();
    register_file(
        &state,
        &context,
        file_id,
        &path,
        Revision::new(1),
        "hash-one",
    )
    .await;
    let repository = state.semantic_memory();
    let (source, revision, _) = repository
        .upsert_source_revision(&context, file_id, &path, Revision::new(1), "hash-one", 0)
        .await
        .unwrap();
    let extraction_id = ExtractionSetId::new();
    repository
        .start_extraction(
            &context,
            extraction_id,
            source.source_id,
            revision.source_revision_id,
            "hash-one",
            "semantic-memory-m1-v1",
            "stale-snapshot",
            "stale-snapshot-request",
        )
        .await
        .unwrap();
    let observation_id = ObservationId::new();
    let evidence_id = EvidenceRefId::new();
    let prepared = repository
        .prepare_publication(
            &context,
            extraction_id,
            &[observation(observation_id, evidence_id, "stale")],
            &[card(
                MemoryCardId::new(),
                CardRevisionId::new(),
                "stale-card",
                None,
                SemanticCardItemInput {
                    id: CardItemId::new(),
                    observation_id,
                    kind: "core_assertion".to_owned(),
                    ordinal: 0,
                    content: "stale".to_owned(),
                    evidence_ref_ids: vec![evidence_id],
                },
            )],
        )
        .await
        .unwrap();
    let snapshot = prepared.first().unwrap();
    repository
        .mark_snapshot_written(
            &context,
            snapshot.id,
            file_id,
            snapshot.proposed_file_revision,
        )
        .await
        .unwrap();

    assert!(
        repository
            .invalidate_source(&context, file_id, "permission_revoked", Some(1))
            .await
            .unwrap()
    );
    assert!(
        repository
            .apply_publication(&context, extraction_id)
            .await
            .is_err()
    );
    assert!(
        repository
            .get_card(&context, snapshot.card_id)
            .await
            .unwrap()
            .is_none()
    );
    let pending = repository
        .pending_snapshots(&context, extraction_id)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].status, "written");
}

#[tokio::test]
async fn semantic_card_target_splits_on_item_content_and_order_changes() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context("target-item-material");
    state
        .vaults()
        .insert(&context, "target-item-material", VaultStatus::Active)
        .await
        .unwrap();
    let file_id = FileId::new();
    let path = VaultPath::parse("notes/target-item-material.md").unwrap();
    register_file(
        &state,
        &context,
        file_id,
        &path,
        Revision::new(1),
        "target-hash",
    )
    .await;
    let repository = state.semantic_memory();
    let (source, revision, _) = repository
        .upsert_source_revision(&context, file_id, &path, Revision::new(1), "target-hash", 0)
        .await
        .unwrap();

    let (first, _) = repository
        .start_extraction(
            &context,
            ExtractionSetId::new(),
            source.source_id,
            revision.source_revision_id,
            &source.content_hash,
            "semantic-memory-m1-v1",
            "target-material-first",
            "target-material-first-request",
        )
        .await
        .unwrap();
    let first_observation = ObservationId::new();
    let first_evidence = EvidenceRefId::new();
    repository
        .prepare_publication(
            &context,
            first.id,
            &[observation(first_observation, first_evidence, "first")],
            &[card(
                MemoryCardId::new(),
                CardRevisionId::new(),
                "first-target",
                None,
                SemanticCardItemInput {
                    id: CardItemId::new(),
                    observation_id: first_observation,
                    kind: "core_assertion".to_owned(),
                    ordinal: 0,
                    content: "first rendered item".to_owned(),
                    evidence_ref_ids: vec![first_evidence],
                },
            )],
        )
        .await
        .unwrap();

    let (second, _) = repository
        .start_extraction(
            &context,
            ExtractionSetId::new(),
            source.source_id,
            revision.source_revision_id,
            &source.content_hash,
            "semantic-memory-m1-v1",
            "target-material-second",
            "target-material-second-request",
        )
        .await
        .unwrap();
    let second_observation = ObservationId::new();
    let second_evidence = EvidenceRefId::new();
    repository
        .prepare_publication(
            &context,
            second.id,
            &[observation(second_observation, second_evidence, "second")],
            &[card(
                MemoryCardId::new(),
                CardRevisionId::new(),
                "second-target",
                None,
                SemanticCardItemInput {
                    id: CardItemId::new(),
                    observation_id: second_observation,
                    kind: "core_assertion".to_owned(),
                    ordinal: 1,
                    content: "second rendered item".to_owned(),
                    evidence_ref_ids: vec![second_evidence],
                },
            )],
        )
        .await
        .unwrap();
    assert_eq!(
        state
            .semantic_rules()
            .list_targets(&context)
            .await
            .unwrap()
            .into_iter()
            .filter(|target| target.target_kind == "memory_card")
            .count(),
        2
    );
}

#[tokio::test]
async fn newer_extraction_fences_older_empty_and_publication_attempts() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context("commit-fence");
    state
        .vaults()
        .insert(&context, "commit-fence", VaultStatus::Active)
        .await
        .unwrap();
    let file_id = FileId::new();
    let path = VaultPath::parse("notes/source.md").unwrap();
    register_file(
        &state,
        &context,
        file_id,
        &path,
        Revision::new(1),
        "hash-one",
    )
    .await;
    let repository = state.semantic_memory();
    let (source, revision, _) = repository
        .upsert_source_revision(&context, file_id, &path, Revision::new(1), "hash-one", 0)
        .await
        .unwrap();
    let (first, _) = repository
        .start_extraction(
            &context,
            ExtractionSetId::new(),
            source.source_id,
            revision.source_revision_id,
            "hash-one",
            "semantic-memory-m1-v1",
            "commit-fence-a",
            "commit-fence-a-request",
        )
        .await
        .unwrap();
    let (second, _) = repository
        .start_extraction(
            &context,
            ExtractionSetId::new(),
            source.source_id,
            revision.source_revision_id,
            "hash-one",
            "semantic-memory-m1-v1",
            "commit-fence-b",
            "commit-fence-b-request",
        )
        .await
        .unwrap();
    assert!(second.extraction_commit_sequence > first.extraction_commit_sequence);

    assert!(
        repository
            .finish_empty_or_nonpublishable(&context, first.id, "success_empty", None)
            .await
            .is_err()
    );

    let observation_id = ObservationId::new();
    let evidence_id = EvidenceRefId::new();
    let stale_prepare = repository
        .prepare_publication(
            &context,
            first.id,
            &[observation(observation_id, evidence_id, "stale")],
            &[card(
                MemoryCardId::new(),
                CardRevisionId::new(),
                "stale-card",
                None,
                SemanticCardItemInput {
                    id: CardItemId::new(),
                    observation_id,
                    kind: "core_assertion".to_owned(),
                    ordinal: 0,
                    content: "stale".to_owned(),
                    evidence_ref_ids: vec![evidence_id],
                },
            )],
        )
        .await;
    assert!(stale_prepare.is_err());
    assert!(
        repository
            .get_extraction(&context, first.id)
            .await
            .unwrap()
            .unwrap()
            .state
            == "running"
    );

    let current_observation_id = ObservationId::new();
    let current_evidence_id = EvidenceRefId::new();
    let prepared = repository
        .prepare_publication(
            &context,
            second.id,
            &[observation(
                current_observation_id,
                current_evidence_id,
                "current",
            )],
            &[card(
                MemoryCardId::new(),
                CardRevisionId::new(),
                "current-card",
                None,
                SemanticCardItemInput {
                    id: CardItemId::new(),
                    observation_id: current_observation_id,
                    kind: "core_assertion".to_owned(),
                    ordinal: 0,
                    content: "current".to_owned(),
                    evidence_ref_ids: vec![current_evidence_id],
                },
            )],
        )
        .await
        .unwrap();
    let snapshot = prepared.first().unwrap();
    repository
        .mark_snapshot_written(
            &context,
            snapshot.id,
            file_id,
            snapshot.proposed_file_revision,
        )
        .await
        .unwrap();
    repository
        .apply_publication(&context, second.id)
        .await
        .unwrap();
    assert!(
        repository
            .get_card(&context, snapshot.card_id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        repository
            .apply_publication(&context, first.id)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn completed_task_rejects_evidence_from_older_same_revision_extraction() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context("task-current-extraction");
    state
        .vaults()
        .insert(&context, "task-current-extraction", VaultStatus::Active)
        .await
        .unwrap();
    let file_id = FileId::new();
    let path = VaultPath::parse("notes/task.md").unwrap();
    register_file(
        &state,
        &context,
        file_id,
        &path,
        Revision::new(1),
        "task-hash",
    )
    .await;
    let repository = state.semantic_memory();
    let (source, revision, _) = repository
        .upsert_source_revision(&context, file_id, &path, Revision::new(1), "task-hash", 0)
        .await
        .unwrap();

    let first_observation_id = ObservationId::new();
    let first_evidence_id = EvidenceRefId::new();
    let (first, _) = repository
        .start_extraction(
            &context,
            ExtractionSetId::new(),
            source.source_id,
            revision.source_revision_id,
            "task-hash",
            "semantic-memory-m1-v1",
            "task-current-first",
            "task-current-first-request",
        )
        .await
        .unwrap();
    let first_card = card(
        MemoryCardId::new(),
        CardRevisionId::new(),
        "task-first",
        None,
        SemanticCardItemInput {
            id: CardItemId::new(),
            observation_id: first_observation_id,
            kind: "core_assertion".to_owned(),
            ordinal: 0,
            content: "Keep the current task evidence.".to_owned(),
            evidence_ref_ids: vec![first_evidence_id],
        },
    );
    repository
        .prepare_publication(
            &context,
            first.id,
            &[observation(
                first_observation_id,
                first_evidence_id,
                "first",
            )],
            &[first_card],
        )
        .await
        .unwrap();
    let first_snapshot = repository
        .pending_snapshots(&context, first.id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    repository
        .mark_snapshot_written(
            &context,
            first_snapshot.id,
            file_id,
            first_snapshot.proposed_file_revision,
        )
        .await
        .unwrap();
    repository
        .apply_publication(&context, first.id)
        .await
        .unwrap();

    let second_observation_id = ObservationId::new();
    let second_evidence_id = EvidenceRefId::new();
    let (second, _) = repository
        .start_extraction(
            &context,
            ExtractionSetId::new(),
            source.source_id,
            revision.source_revision_id,
            "task-hash",
            "semantic-memory-m1-v1",
            "task-current-second",
            "task-current-second-request",
        )
        .await
        .unwrap();
    let second_card = card(
        MemoryCardId::new(),
        CardRevisionId::new(),
        "task-second",
        None,
        SemanticCardItemInput {
            id: CardItemId::new(),
            observation_id: second_observation_id,
            kind: "core_assertion".to_owned(),
            ordinal: 0,
            content: "Keep the current task evidence.".to_owned(),
            evidence_ref_ids: vec![second_evidence_id],
        },
    );
    repository
        .prepare_publication(
            &context,
            second.id,
            &[observation(
                second_observation_id,
                second_evidence_id,
                "second",
            )],
            &[second_card],
        )
        .await
        .unwrap();
    let second_snapshot = repository
        .pending_snapshots(&context, second.id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    repository
        .mark_snapshot_written(
            &context,
            second_snapshot.id,
            file_id,
            second_snapshot.proposed_file_revision,
        )
        .await
        .unwrap();
    repository
        .apply_publication(&context, second.id)
        .await
        .unwrap();

    let rules = state.semantic_rules();
    assert!(
        rules
            .set_task_state(
                &context,
                "task-old-evidence",
                "task",
                "project",
                1,
                "task-old-target",
                "completed",
                Some(&first_observation_id.to_string()),
                Some(&source.source_id.to_string()),
                Some(&revision.source_revision_id.to_string()),
                None,
            )
            .await
            .is_err()
    );
    assert!(
        rules
            .set_task_state(
                &context,
                "task-current-evidence",
                "task",
                "project",
                1,
                "task-current-target",
                "completed",
                Some(&second_observation_id.to_string()),
                Some(&source.source_id.to_string()),
                Some(&revision.source_revision_id.to_string()),
                None,
            )
            .await
            .is_ok()
    );
}
