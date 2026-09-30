use std::path::PathBuf;

use mcp_vault_domain::{
    Actor, ActorType, CardItemId, CardRevisionId, ComposedCardId, ComposedCardItemId,
    ComposedCardRevisionId, EvidenceRefId, ExtractionSetId, FileId, MemoryCardId, ObservationId,
    OperationId, OrganizationJobId, RelationCandidateId, RelationDecisionId, Revision, SourcePlane,
    SourceRevisionId, SupportGroupId, SupportMemberId, VaultContext, VaultId, VaultPath, VaultSlug,
};
use mcp_vault_state::{
    CommitMutationInput, ComposedCardItemInput, ComposedCardRevisionInput, EntryType,
    FileOperation, NoopCommitHook, OrganizationSourceFence, PrepareOperationInput,
    RelationCandidateInput, RelationDecisionInput, RelationEvidenceInput, SemanticCardItemInput,
    SemanticCardRevisionInput, SemanticEvidenceInput, SemanticObservationInput, SemanticSpanInput,
    StateStore, SupportGroupInput, SupportMemberInput, VaultStatus,
};
use serde_json::json;
use sha2::{Digest, Sha256};

fn context(slug: &str) -> VaultContext {
    VaultContext::new(
        VaultId::new(),
        VaultSlug::new(slug).unwrap(),
        PathBuf::from(format!("/tmp/mcp-vault-organization-{slug}")),
        Revision::ZERO,
    )
    .unwrap()
}

async fn register_file(
    state: &StateStore,
    context: &VaultContext,
    file_id: FileId,
    path: &VaultPath,
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
                proposed_hash: Some("source-hash".to_owned()),
                temp_path: Some(VaultPath::parse("_mcp-vault/.tmp/organization-test").unwrap()),
                payload: json!({"test": "organization"}),
                idempotency_key: None,
            },
        )
        .await
        .unwrap();
    state
        .files()
        .mark_file_committed(context, operation_id, Some("source-hash"))
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
                content_hash: Some("source-hash".to_owned()),
                history_blob_hash: None,
                size: 1,
                modified_at: 1,
                filesystem_identity: None,
                deleted_at: None,
                operation: FileOperation::Create,
                actor: Actor::new(ActorType::System, None),
                source_plane: SourcePlane::System,
                idempotency_key: None,
                audit_action: "test.semantic_organization_source".to_owned(),
                audit_metadata: json!({}),
                request_id: None,
                outbox_events: Vec::new(),
            },
            &NoopCommitHook,
        )
        .await
        .unwrap();
}

async fn materialize_source(
    state: &StateStore,
    context: &VaultContext,
    ordinal: u8,
) -> (
    mcp_vault_state::SemanticSourceRecord,
    ObservationId,
    EvidenceRefId,
) {
    let file_id = FileId::new();
    let path = VaultPath::parse(&format!("notes/source-{ordinal}.md")).unwrap();
    register_file(state, context, file_id, &path).await;
    let repository = state.semantic_memory();
    let (source, revision, _) = repository
        .upsert_source_revision(context, file_id, &path, Revision::new(1), "source-hash", 0)
        .await
        .unwrap();
    let extraction_id = ExtractionSetId::new();
    repository
        .start_extraction(
            context,
            extraction_id,
            source.source_id,
            revision.source_revision_id,
            "source-hash",
            "semantic-memory-m1-v1",
            &format!("organization-source-{ordinal}"),
            &format!("organization-source-request-{ordinal}"),
        )
        .await
        .unwrap();
    let observation_id = ObservationId::new();
    let evidence_id = EvidenceRefId::new();
    let observation = SemanticObservationInput {
        id: observation_id,
        local_key: "observation".to_owned(),
        kind: "decision".to_owned(),
        statement: format!("Source {ordinal} supports the bounded decision."),
        scope: "project".to_owned(),
        assertion_status: "source_asserted".to_owned(),
        source_time_scope: json!({}),
        conditions: Vec::new(),
        exceptions: Vec::new(),
        ordered_steps: Vec::new(),
        result: None,
        uncertainty: None,
        admission_reason: "organization test".to_owned(),
        value_for_future_work: "preserve the source support".to_owned(),
        evidence: SemanticEvidenceInput {
            id: evidence_id,
            body_spans: vec![SemanticSpanInput {
                start_byte: 0,
                end_byte: 1,
                content_hash: "span".to_owned(),
            }],
            context_spans: Vec::new(),
        },
    };
    let card_id = MemoryCardId::new();
    let card_revision_id = CardRevisionId::new();
    repository
        .prepare_publication(
            context,
            extraction_id,
            std::slice::from_ref(&observation),
            &[SemanticCardRevisionInput {
                card_id,
                card_revision_id,
                expected_card_revision_id: None,
                revision_number: 1,
                topic_key: format!("source-{ordinal}"),
                title: format!("Source {ordinal}"),
                kind: "decision".to_owned(),
                scope_ref: "project".to_owned(),
                assertion_status: "source_asserted".to_owned(),
                temporal_scope: json!({}),
                composition_profile_id: "semantic-memory-m1-v1".to_owned(),
                canonical_path: VaultPath::parse(&format!("_mcp-vault/source-{ordinal}.md"))
                    .unwrap(),
                canonical_markdown_hash: "card-hash".to_owned(),
                canonical_bytes: b"card".to_vec(),
                items: vec![SemanticCardItemInput {
                    id: CardItemId::new(),
                    observation_id,
                    kind: "core_assertion".to_owned(),
                    ordinal: 0,
                    content: observation.statement.clone(),
                    evidence_ref_ids: vec![evidence_id],
                }],
            }],
        )
        .await
        .unwrap();
    let snapshots = repository
        .pending_snapshots(context, extraction_id)
        .await
        .unwrap();
    repository
        .mark_snapshot_written(context, snapshots[0].id, file_id, Revision::new(1))
        .await
        .unwrap();
    repository
        .apply_publication(context, extraction_id)
        .await
        .unwrap();
    (
        repository
            .get_source(context, source.source_id)
            .await
            .unwrap()
            .unwrap(),
        observation_id,
        evidence_id,
    )
}

fn fence(source: &mcp_vault_state::SemanticSourceRecord) -> OrganizationSourceFence {
    OrganizationSourceFence {
        source_id: source.source_id,
        source_revision_id: source.current_revision_id.unwrap(),
        content_hash: source.content_hash.clone(),
        authorization_revision: source.authorization_revision,
        source_generation: source.source_generation,
        extraction_commit_sequence: source.extraction_commit_sequence,
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn card_input(
    card_id: ComposedCardId,
    revision_id: ComposedCardRevisionId,
    first: (
        &mcp_vault_state::SemanticSourceRecord,
        ObservationId,
        EvidenceRefId,
    ),
    second: (
        &mcp_vault_state::SemanticSourceRecord,
        ObservationId,
        EvidenceRefId,
    ),
    operator: &str,
) -> ComposedCardRevisionInput {
    let bytes = b"composed card".to_vec();
    let members = [first, second]
        .into_iter()
        .map(
            |(source, observation_id, evidence_ref_id)| SupportMemberInput {
                id: SupportMemberId::new(),
                source_id: source.source_id,
                source_revision_id: source.current_revision_id.unwrap(),
                observation_id,
                evidence_ref_id,
                member_role: "complete".to_owned(),
            },
        )
        .collect::<Vec<_>>();
    ComposedCardRevisionInput {
        card_id,
        card_revision_id: revision_id,
        expected_card_revision_id: None,
        revision_number: 1,
        identity_key: "bounded-decision".to_owned(),
        topic_key: "bounded-decision".to_owned(),
        title: "Bounded decision".to_owned(),
        kind: "decision".to_owned(),
        scope_ref: "project".to_owned(),
        assertion_status: "source_asserted".to_owned(),
        temporal_scope: json!({}),
        composition_profile_id: "semantic-memory-m2-v1".to_owned(),
        canonical_path: VaultPath::parse("_mcp-vault/semantic-memory/composed/bounded.md").unwrap(),
        canonical_markdown_hash: digest(&bytes),
        canonical_bytes: bytes,
        dependencies: vec![
            (first.0.source_id, first.0.current_revision_id.unwrap()),
            (second.0.source_id, second.0.current_revision_id.unwrap()),
        ],
        items: vec![ComposedCardItemInput {
            id: ComposedCardItemId::new(),
            kind: "core_assertion".to_owned(),
            ordinal: 0,
            content: "The bounded decision is supported by the selected sources.".to_owned(),
            support_groups: vec![SupportGroupInput {
                id: SupportGroupId::new(),
                operator: operator.to_owned(),
                ordinal: 0,
                members,
            }],
        }],
    }
}

#[tokio::test]
async fn organization_jobs_are_vault_scoped_and_idempotent() {
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
    let (source, _, _) = materialize_source(&state, &first, 1).await;
    let repository = state.semantic_organization();
    let id = OrganizationJobId::new();
    let (job, created) = repository
        .start_job(
            &first,
            id,
            "input-one",
            1,
            "m2",
            "same-key",
            "request-one",
            &[fence(&source)],
        )
        .await
        .unwrap();
    assert!(created);
    let (retry, created) = repository
        .start_job(
            &first,
            OrganizationJobId::new(),
            "input-one",
            1,
            "m2",
            "same-key",
            "request-one",
            &[fence(&source)],
        )
        .await
        .unwrap();
    assert!(!created);
    assert_eq!(retry.id, job.id);
    assert!(
        repository
            .start_job(
                &first,
                OrganizationJobId::new(),
                "input-two",
                1,
                "m2",
                "same-key",
                "request-two",
                &[fence(&source)]
            )
            .await
            .is_err()
    );
    let different_source = OrganizationSourceFence {
        source_id: mcp_vault_domain::SemanticSourceId::new(),
        source_revision_id: mcp_vault_domain::SourceRevisionId::new(),
        content_hash: "other".to_owned(),
        authorization_revision: 0,
        source_generation: 0,
        extraction_commit_sequence: 0,
    };
    assert!(
        repository
            .start_job(
                &first,
                OrganizationJobId::new(),
                "input-one",
                1,
                "m2",
                "same-key",
                "request-one",
                &[different_source],
            )
            .await
            .is_err()
    );
    assert!(
        repository
            .start_job(
                &second,
                OrganizationJobId::new(),
                "input-one",
                1,
                "m2",
                "other-key",
                "other-request",
                &[fence(&source)]
            )
            .await
            .is_err()
    );
    assert_eq!(repository.list_jobs(&second, 20).await.unwrap().len(), 0);
}

#[tokio::test]
async fn running_organization_cancellation_is_terminal_and_vault_scoped() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("organization-cancel.sqlite3");
    let url = format!("sqlite://{}", database.display());
    let state = StateStore::connect_and_migrate(&url).await.unwrap();
    let first = context("organization-cancel-first");
    let second = context("organization-cancel-second");
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
    let (source, source_observation, source_evidence) = materialize_source(&state, &first, 9).await;
    let job_id = OrganizationJobId::new();
    state
        .semantic_organization()
        .start_job(
            &first,
            job_id,
            "cancel-input",
            0,
            "semantic-memory-m2-v1",
            "cancel-idempotency",
            "cancel-request",
            &[fence(&source)],
        )
        .await
        .unwrap();
    state
        .semantic_organization()
        .cancel_job(&first, job_id, "semantic_organization_cancelled")
        .await
        .unwrap();

    let (second_source, second_observation, second_evidence) =
        materialize_source(&state, &first, 10).await;
    for (job_id, key) in [
        (OrganizationJobId::new(), "prepared-cancel"),
        (OrganizationJobId::new(), "written-cancel"),
    ] {
        state
            .semantic_organization()
            .start_job(
                &first,
                job_id,
                key,
                0,
                "semantic-memory-m2-v1",
                &format!("{key}-idempotency"),
                &format!("{key}-request"),
                &[fence(&source), fence(&second_source)],
            )
            .await
            .unwrap();
        let mut cancel_card = card_input(
            ComposedCardId::new(),
            ComposedCardRevisionId::new(),
            (&source, source_observation, source_evidence),
            (&second_source, second_observation, second_evidence),
            "or",
        );
        cancel_card.identity_key = key.to_owned();
        cancel_card.topic_key = key.to_owned();
        let snapshots = state
            .semantic_organization()
            .prepare_organization(&first, job_id, &[], &[cancel_card])
            .await
            .unwrap();
        if key == "written-cancel" {
            state
                .semantic_organization()
                .mark_snapshot_written(
                    &first,
                    snapshots[0].id,
                    source.file_id,
                    Revision::new(1),
                    &snapshots[0].proposed_file_hash,
                )
                .await
                .unwrap();
        }
        assert!(
            state
                .semantic_organization()
                .cancel_job(&first, job_id, "semantic_organization_cancelled")
                .await
                .is_err()
        );
        assert_eq!(
            state
                .semantic_organization()
                .get_job(&first, job_id)
                .await
                .unwrap()
                .unwrap()
                .lifecycle_state,
            if key == "written-cancel" {
                "written"
            } else {
                "prepared"
            }
        );
    }
    state.close().await;

    let reopened = StateStore::connect_and_migrate(&url).await.unwrap();
    let job = reopened
        .semantic_organization()
        .get_job(&first, job_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.lifecycle_state, "cancelled");
    assert!(
        !reopened
            .semantic_organization()
            .pending_jobs(&first)
            .await
            .unwrap()
            .contains(&job_id)
    );
    assert!(
        reopened
            .semantic_organization()
            .get_job(&second, job_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        reopened
            .semantic_organization()
            .cancel_job(&first, job_id, "semantic_organization_cancelled")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn stale_source_fence_is_rejected_before_composed_write() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context("source-prewrite-fence");
    state
        .vaults()
        .insert(&context, "source-prewrite-fence", VaultStatus::Active)
        .await
        .unwrap();
    let (source, _, _) = materialize_source(&state, &context, 1).await;
    let repository = state.semantic_organization();
    let job_id = OrganizationJobId::new();
    repository
        .start_job(
            &context,
            job_id,
            "source-prewrite",
            0,
            "m2",
            "source-prewrite-key",
            "source-prewrite-request",
            &[fence(&source)],
        )
        .await
        .unwrap();
    state
        .semantic_memory()
        .invalidate_source(&context, source.file_id, "source_changed", None)
        .await
        .unwrap();
    assert!(
        repository
            .validate_job_write_fence(&context, job_id)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn candidate_is_not_accepted_and_composed_support_is_and_or_scoped() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context("relations");
    state
        .vaults()
        .insert(&context, "relations", VaultStatus::Active)
        .await
        .unwrap();
    let first = materialize_source(&state, &context, 1).await;
    let second = materialize_source(&state, &context, 2).await;
    let repository = state.semantic_organization();
    let job_id = OrganizationJobId::new();
    repository
        .start_job(
            &context,
            job_id,
            "input",
            1,
            "m2",
            "job-key",
            "job-request",
            &[fence(&first.0), fence(&second.0)],
        )
        .await
        .unwrap();
    let candidate_id = RelationCandidateId::new();
    repository
        .record_candidate(
            &context,
            job_id,
            &RelationCandidateInput {
                id: candidate_id,
                left_observation_id: first.1,
                left_source_id: first.0.source_id,
                left_source_revision_id: first.0.current_revision_id.unwrap(),
                right_observation_id: second.1,
                right_source_id: second.0.source_id,
                right_source_revision_id: second.0.current_revision_id.unwrap(),
                candidate_input_hash: "candidate".to_owned(),
                similarity_hint: Some(99),
                profile_id: "m2".to_owned(),
            },
        )
        .await
        .unwrap();
    let candidate_only = repository.get_job(&context, job_id).await.unwrap().unwrap();
    assert_eq!(candidate_only.state, "running");
    let decision = RelationDecisionInput {
        id: RelationDecisionId::new(),
        candidate_id,
        relation_kind: "equivalent".to_owned(),
        decision_state: "accepted".to_owned(),
        decision_reason_code: "same-bounded-decision".to_owned(),
        profile_id: "m2".to_owned(),
        evidence: vec![RelationEvidenceInput {
            evidence_ref_id: first.2,
            source_id: first.0.source_id,
            source_revision_id: first.0.current_revision_id.unwrap(),
            role: "left".to_owned(),
        }],
    };
    let card = card_input(
        ComposedCardId::new(),
        ComposedCardRevisionId::new(),
        (&first.0, first.1, first.2),
        (&second.0, second.1, second.2),
        "and",
    );
    let snapshots = repository
        .prepare_organization(&context, job_id, &[decision], &[card])
        .await
        .unwrap();
    assert_eq!(snapshots.len(), 1);
    let cards = repository.list_composed_cards(&context, 20).await.unwrap();
    assert!(
        cards.is_empty(),
        "prepared cards are not readable before Core publication"
    );
    state
        .semantic_rules()
        .apply_correction(
            &context,
            "organization_job",
            "project",
            1,
            "rules-fence-test",
            &json!({"replace":"bounded decision","remove":"original decision"}),
            None,
            None,
            None,
            None,
            None,
            Some(0),
            "test-actor",
            "rules-fence-test",
        )
        .await
        .unwrap();
    assert!(
        repository
            .mark_snapshot_written(
                &context,
                snapshots[0].id,
                first.0.file_id,
                Revision::new(1),
                &snapshots[0].proposed_file_hash,
            )
            .await
            .is_err()
    );
    assert!(
        repository
            .apply_organization(&context, job_id)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cross_revision_input_rolls_back_and_source_invalidation_hides_card() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context("qualification");
    state
        .vaults()
        .insert(&context, "qualification", VaultStatus::Active)
        .await
        .unwrap();
    let first = materialize_source(&state, &context, 1).await;
    let second = materialize_source(&state, &context, 2).await;
    let repository = state.semantic_organization();
    let job_id = OrganizationJobId::new();
    repository
        .start_job(
            &context,
            job_id,
            "input",
            1,
            "m2",
            "qualification-job",
            "qualification-request",
            &[fence(&first.0), fence(&second.0)],
        )
        .await
        .unwrap();
    let bad_card = card_input(
        ComposedCardId::new(),
        ComposedCardRevisionId::new(),
        (&first.0, first.1, first.2),
        (&second.0, second.1, second.2),
        "or",
    );
    let mut bad_card = bad_card;
    bad_card.dependencies[0].1 = SourceRevisionId::new();
    assert!(
        repository
            .prepare_organization(&context, job_id, &[], &[bad_card])
            .await
            .is_err()
    );
    assert!(
        repository
            .list_composed_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );

    let good_card = card_input(
        ComposedCardId::new(),
        ComposedCardRevisionId::new(),
        (&first.0, first.1, first.2),
        (&second.0, second.1, second.2),
        "or",
    );
    let snapshots = repository
        .prepare_organization(&context, job_id, &[], &[good_card])
        .await
        .unwrap();
    assert_eq!(snapshots[0].status, "prepared");
    let snapshot = snapshots[0].clone();
    assert!(
        repository
            .mark_snapshot_written(
                &context,
                snapshot.id,
                first.0.file_id,
                Revision::new(2),
                &snapshot.proposed_file_hash,
            )
            .await
            .is_err()
    );
    assert!(
        repository
            .mark_snapshot_written(
                &context,
                snapshot.id,
                first.0.file_id,
                Revision::new(1),
                "wrong-hash",
            )
            .await
            .is_err()
    );
    repository
        .mark_snapshot_written(
            &context,
            snapshot.id,
            first.0.file_id,
            Revision::new(1),
            &snapshot.proposed_file_hash,
        )
        .await
        .unwrap();
    repository
        .apply_organization(&context, job_id)
        .await
        .unwrap();
    assert_eq!(
        repository
            .list_composed_cards(&context, 20)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        state
            .semantic_memory()
            .invalidate_source(&context, first.0.file_id, "source_changed", None)
            .await
            .unwrap()
    );
    assert_eq!(
        repository
            .list_composed_cards(&context, 20)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        state
            .semantic_memory()
            .invalidate_source(&context, second.0.file_id, "source_changed", None)
            .await
            .unwrap()
    );
    assert!(
        repository
            .list_composed_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repository
            .start_job(
                &context,
                OrganizationJobId::new(),
                "input",
                1,
                "m2",
                "qualification-job",
                "qualification-request",
                &[fence(&first.0), fence(&second.0)],
            )
            .await
            .is_err(),
        "an idempotent replay must revalidate invalidated source fences"
    );
}

#[tokio::test]
async fn failed_organization_terminalizes_candidates_and_audits_atomically() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context("terminal-lifecycle");
    state
        .vaults()
        .insert(&context, "terminal-lifecycle", VaultStatus::Active)
        .await
        .unwrap();
    let first = materialize_source(&state, &context, 1).await;
    let second = materialize_source(&state, &context, 2).await;
    let repository = state.semantic_organization();
    let job_id = OrganizationJobId::new();
    repository
        .start_job(
            &context,
            job_id,
            "terminal-input",
            1,
            "m2",
            "terminal-job",
            "terminal-request",
            &[fence(&first.0), fence(&second.0)],
        )
        .await
        .unwrap();
    let candidate_id = RelationCandidateId::new();
    repository
        .record_candidate(
            &context,
            job_id,
            &RelationCandidateInput {
                id: candidate_id,
                left_observation_id: first.1,
                left_source_id: first.0.source_id,
                left_source_revision_id: first.0.current_revision_id.unwrap(),
                right_observation_id: second.1,
                right_source_id: second.0.source_id,
                right_source_revision_id: second.0.current_revision_id.unwrap(),
                candidate_input_hash: "terminal-candidate".to_owned(),
                similarity_hint: None,
                profile_id: "m2".to_owned(),
            },
        )
        .await
        .unwrap();
    repository
        .record_action_audits(
            &context,
            job_id,
            &[mcp_vault_state::OrganizationActionAuditInput {
                ordinal: 0,
                action_kind: "create_composed_card".to_owned(),
                candidate_ids: vec![candidate_id],
                reason: Some("terminal test".to_owned()),
            }],
        )
        .await
        .unwrap();
    let mut invalid_card = card_input(
        ComposedCardId::new(),
        ComposedCardRevisionId::new(),
        (&first.0, first.1, first.2),
        (&second.0, second.1, second.2),
        "or",
    );
    invalid_card.dependencies[0].1 = SourceRevisionId::new();
    assert!(
        repository
            .prepare_organization(&context, job_id, &[], &[invalid_card])
            .await
            .is_err()
    );
    repository
        .mark_job_failed(&context, job_id, "semantic_prepare_failed", false)
        .await
        .unwrap();
    assert_eq!(
        repository
            .organization_candidate_states(&context, job_id)
            .await
            .unwrap(),
        vec!["rejected".to_owned()]
    );
    assert_eq!(
        repository
            .organization_action_outcomes(&context, job_id)
            .await
            .unwrap(),
        vec!["blocked".to_owned()]
    );
    assert_eq!(
        repository
            .get_job(&context, job_id)
            .await
            .unwrap()
            .unwrap()
            .lifecycle_state,
        "failed"
    );
}
