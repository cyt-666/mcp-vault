use mcp_vault_core::{CommitPhase, FailureInjector, VaultCore};
use mcp_vault_domain::{
    Actor, EvidenceRefId, ExtractionSetId, OrganizationJobId, Revision, SourcePlane, VaultContext,
    VaultId, VaultPath, VaultPathPolicy, VaultSlug, WritePrecondition,
};
use mcp_vault_memory::{SemanticMemoryService, semantic::organize::SemanticOrganizationService};
use mcp_vault_state::{
    SemanticCardItemInput, SemanticCardRecord, SemanticCardRevisionInput, SemanticEvidenceInput,
    SemanticObservationInput, SemanticSpanInput, StateError, StateStore, VaultStatus,
};
use mcp_vault_storage_fs::{DurabilityPolicy, StorageOptions};
use serde_json::json;
use sqlx::SqlitePool;
use std::fs;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

struct FailOnce {
    phase: CommitPhase,
    fired: AtomicBool,
}

impl FailureInjector for FailOnce {
    fn fail(&self, phase: CommitPhase) -> Result<(), &'static str> {
        if phase == self.phase && !self.fired.swap(true, Ordering::SeqCst) {
            Err("semantic M1 recovery fixture fault")
        } else {
            Ok(())
        }
    }
}

async fn fixture() -> (
    tempfile::TempDir,
    StateStore,
    VaultContext,
    VaultCore,
    SemanticMemoryService,
) {
    let dir = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("semantic").unwrap(),
        dir.path().join("vault"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&context, "semantic", VaultStatus::Active)
        .await
        .unwrap();
    state
        .settings()
        .set_vault(
            &context,
            "memory.units.policy",
            &json!({"enabled":true,"request_timeout_seconds":300}),
            WritePrecondition::Unconditional,
            None,
        )
        .await
        .unwrap();
    let core = VaultCore::new(
        state.clone(),
        dir.path().join("history"),
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
        Default::default(),
    );
    let service = SemanticMemoryService::new(state.clone());
    (dir, state, context, core, service)
}

async fn file_backed_fixture() -> (
    tempfile::TempDir,
    StateStore,
    VaultContext,
    VaultCore,
    SemanticMemoryService,
    String,
) {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("semantic-corruption.sqlite3");
    let url = format!("sqlite://{}", database.display());
    let state = StateStore::connect_and_migrate(&url).await.unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("semantic-corruption").unwrap(),
        dir.path().join("vault"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&context, "semantic-corruption", VaultStatus::Active)
        .await
        .unwrap();
    state
        .settings()
        .set_vault(
            &context,
            "memory.units.policy",
            &json!({"enabled":true,"request_timeout_seconds":300}),
            WritePrecondition::Unconditional,
            None,
        )
        .await
        .unwrap();
    let core = VaultCore::new(
        state.clone(),
        dir.path().join("history"),
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
        Default::default(),
    );
    let service = SemanticMemoryService::new(state.clone());
    (dir, state, context, core, service, url)
}

async fn publish_decision(
    service: &SemanticMemoryService,
    context: &VaultContext,
    core: &VaultCore,
    path: &VaultPath,
    statement: &str,
    title: &str,
) -> (SemanticCardRecord, EvidenceRefId) {
    let input = service.prepare_source(context, core, path).await.unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":statement,"scope":"project","assertion_status":"source_asserted","admission_reason":"source event test","value_for_future_work":"preserve the current evidence","body_block_ids":[body.local_id.clone()]}],
        "cards":[{"title":title,"kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    service
        .submit_proposal_json(context, core, path, &proposal.to_string())
        .await
        .unwrap();
    let card = service
        .list_cards(context, core, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let evidence_id = card.items[0].evidence_ref_ids[0];
    (card, evidence_id)
}

#[tokio::test]
async fn reconcile_source_event_tracks_current_file_and_policy() {
    let (_dir, state, context, core, service) = fixture().await;
    let old_path = VaultPath::parse("notes/event.md").unwrap();
    let new_path = VaultPath::parse("archive/event.md").unwrap();
    core.create_bytes(
        &context,
        &old_path,
        b"# Event\nThe original source remains current.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let initial_file = core.read(&context, &old_path).await.unwrap().file;
    let (initial_card, _) = publish_decision(
        &service,
        &context,
        &core,
        &old_path,
        "The original source remains current.",
        "Original source",
    )
    .await;
    let initial_source = state
        .semantic_memory()
        .get_source(&context, initial_card.source_id)
        .await
        .unwrap()
        .unwrap();
    let repeated = service
        .reconcile_source_event(&context, &core, initial_file.id)
        .await
        .unwrap();
    assert_eq!(
        repeated.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::NavigationOnly
    );
    assert_eq!(
        repeated.source_generation,
        Some(initial_source.source_generation)
    );

    let moved = core
        .move_entry(
            &context,
            &old_path,
            &new_path,
            initial_file.current_revision,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let moved_report = service
        .reconcile_source_event(&context, &core, moved.file.id)
        .await
        .unwrap();
    assert_eq!(
        moved_report.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::NavigationOnly
    );
    let moved_source = state
        .semantic_memory()
        .get_source(&context, initial_card.source_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(moved_source.source_path, new_path);
    assert_eq!(
        moved_source.source_generation,
        initial_source.source_generation
    );
    assert_eq!(
        service.list_cards(&context, &core, 20).await.unwrap().len(),
        1
    );

    let changed = core
        .replace_bytes(
            &context,
            &new_path,
            moved.file.current_revision,
            b"# Event\nThe source changed and needs rebuilding.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let changed_report = service
        .reconcile_source_event(&context, &core, changed.file.id)
        .await
        .unwrap();
    assert_eq!(
        changed_report.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::NeedsRebuild
    );
    let changed_source = state
        .semantic_memory()
        .get_source(&context, initial_card.source_id)
        .await
        .unwrap()
        .unwrap();
    assert!(!changed_source.eligible);
    assert!(changed_source.pending_rebuild);
    let duplicate_report = service
        .reconcile_source_event(&context, &core, initial_file.id)
        .await
        .unwrap();
    assert_eq!(
        duplicate_report.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::NeedsRebuild
    );
    assert_eq!(
        duplicate_report.file_revision,
        Some(changed.file.current_revision)
    );

    let (changed_card, changed_evidence) = publish_decision(
        &service,
        &context,
        &core,
        &new_path,
        "The source changed and needs rebuilding.",
        "Changed source",
    )
    .await;
    assert!(
        service
            .read_evidence(&context, &core, changed_evidence)
            .await
            .unwrap()
            .is_some()
    );
    let deleted = core
        .delete(
            &context,
            &new_path,
            changed.file.current_revision,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let deleted_report = service
        .reconcile_source_event(&context, &core, deleted.file.id)
        .await
        .unwrap();
    assert_eq!(
        deleted_report.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::Invalidated
    );
    assert!(
        service
            .read_evidence(&context, &core, changed_evidence)
            .await
            .unwrap()
            .is_none()
    );

    let restored = core
        .restore(
            &context,
            &new_path,
            changed.file.current_revision,
            deleted.file.current_revision,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let restored_report = service
        .reconcile_source_event(&context, &core, restored.file.id)
        .await
        .unwrap();
    assert_eq!(
        restored_report.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::NeedsRebuild
    );
    assert!(restored_report.source_id.is_some());
    assert_eq!(
        service
            .get_card(&context, &core, changed_card.id)
            .await
            .unwrap(),
        None
    );

    let policy_revision = state
        .settings()
        .get_vault(&context, "memory.units.policy")
        .await
        .unwrap()
        .unwrap()
        .revision;
    state
        .settings()
        .set_vault(
            &context,
            "memory.units.policy",
            &json!({"enabled":false,"request_timeout_seconds":300}),
            WritePrecondition::ExactRevision(policy_revision),
            None,
        )
        .await
        .unwrap();
    let disabled = service
        .reconcile_source_event(&context, &core, restored.file.id)
        .await
        .unwrap();
    assert_eq!(
        disabled.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::PolicyDisabled
    );

    let disabled_revision = state
        .settings()
        .get_vault(&context, "memory.units.policy")
        .await
        .unwrap()
        .unwrap()
        .revision;
    state
        .settings()
        .set_vault(
            &context,
            "memory.units.policy",
            &json!({"enabled":"invalid","request_timeout_seconds":300}),
            WritePrecondition::ExactRevision(disabled_revision),
            None,
        )
        .await
        .unwrap();
    let invalid = service
        .reconcile_source_event(&context, &core, restored.file.id)
        .await
        .unwrap();
    assert_eq!(
        invalid.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::PolicyInvalid
    );
}

#[tokio::test]
async fn local_rebind_preserves_a_card_when_only_unrelated_content_is_inserted() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/rebind.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Stable context\nKeep the bounded decision.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let (old_card, old_evidence) = publish_decision(
        &service,
        &context,
        &core,
        &path,
        "Keep the bounded decision.",
        "Rebound decision",
    )
    .await;
    let changed = core
        .replace_bytes(
            &context,
            &path,
            core.read(&context, &path)
                .await
                .unwrap()
                .file
                .current_revision,
            b"# Stable context\nAn unrelated paragraph changed.\nKeep the bounded decision.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let report = service
        .reconcile_source_event(&context, &core, changed.file.id)
        .await
        .unwrap();
    assert_eq!(
        report.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::Rebound
    );
    let rebound = service
        .list_cards(&context, &core, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(rebound.id, old_card.id);
    assert!(rebound.revision_number > old_card.revision_number);
    assert_ne!(rebound.source_revision_id, old_card.source_revision_id);
    assert!(
        service
            .read_evidence(&context, &core, old_evidence)
            .await
            .unwrap()
            .is_none()
    );
    let new_evidence = rebound.items[0].evidence_ref_ids[0];
    assert!(
        service
            .read_evidence(&context, &core, new_evidence)
            .await
            .unwrap()
            .is_some()
    );
    let source = state
        .semantic_memory()
        .get_source(&context, old_card.source_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source.current_revision_id, Some(rebound.source_revision_id));
}

#[tokio::test]
async fn local_rebind_fails_closed_when_any_card_is_incomplete() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/rebind-subset.md").unwrap();
    let original = b"# Stable\nKeep this decision.\n\n# Changed\nReplace this decision.\n";
    core.create_bytes(
        &context,
        &path,
        original,
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &path)
        .await
        .unwrap();
    let stable = input
        .blocks
        .iter()
        .find(|block| block.text == "Keep this decision.")
        .unwrap();
    let changed = input
        .blocks
        .iter()
        .find(|block| block.text == "Replace this decision.")
        .unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[
            {"kind":"decision","statement":"Keep this decision.","scope":"project","assertion_status":"source_asserted","admission_reason":"subset test","value_for_future_work":"keep stable","body_block_ids":[stable.local_id.clone()]},
            {"kind":"decision","statement":"Replace this decision.","scope":"project","assertion_status":"source_asserted","admission_reason":"subset test","value_for_future_work":"replace changed","body_block_ids":[changed.local_id.clone()]}
        ],
        "cards":[
            {"title":"Stable card","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]},
            {"title":"Changed card","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[1]}
        ]
    });
    service
        .submit_proposal_json(&context, &core, &path, &proposal.to_string())
        .await
        .unwrap();
    let old_cards = service.list_cards(&context, &core, 20).await.unwrap();
    assert_eq!(old_cards.len(), 2);
    assert!(old_cards.iter().any(|card| card.title == "Stable card"));
    let current = core.read(&context, &path).await.unwrap().file;
    let updated = core
        .replace_bytes(
            &context,
            &path,
            current.current_revision,
            b"# Stable\nKeep this decision.\n\n# Changed\nA different decision.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let report = service
        .reconcile_source_event(&context, &core, updated.file.id)
        .await
        .unwrap();
    assert_eq!(
        report.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::NeedsRebuild
    );
    let current_cards = service.list_cards(&context, &core, 20).await.unwrap();
    assert!(current_cards.is_empty());
    let source = state
        .semantic_memory()
        .get_source_by_file(&context, updated.file.id)
        .await
        .unwrap()
        .unwrap();
    assert!(source.pending_rebuild);
}

#[tokio::test]
async fn local_rebind_fails_closed_for_duplicate_or_changed_context_spans() {
    for (slug, content) in [
        (
            "duplicate-rebind",
            b"# Stable context\nKeep the bounded decision.\nKeep the bounded decision.\n"
                .as_slice(),
        ),
        (
            "context-rebind",
            b"# Changed context\nKeep the bounded decision.\n".as_slice(),
        ),
    ] {
        let (_dir, state, context, core, service) = fixture().await;
        let path = VaultPath::parse(&format!("notes/{slug}.md")).unwrap();
        core.create_bytes(
            &context,
            &path,
            b"# Stable context\nKeep the bounded decision.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        let (old_card, old_evidence) = publish_decision(
            &service,
            &context,
            &core,
            &path,
            "Keep the bounded decision.",
            "Fail-closed rebind",
        )
        .await;
        let current = core.read(&context, &path).await.unwrap().file;
        let changed = core
            .replace_bytes(
                &context,
                &path,
                current.current_revision,
                content,
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await
            .unwrap();
        let report = service
            .reconcile_source_event(&context, &core, changed.file.id)
            .await
            .unwrap();
        assert_eq!(
            report.disposition,
            mcp_vault_memory::SemanticSourceEventDisposition::NeedsRebuild,
            "{slug} must not reuse an ambiguous or changed-context contribution"
        );
        assert!(
            service
                .list_cards(&context, &core, 20)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            service
                .read_evidence(&context, &core, old_evidence)
                .await
                .unwrap()
                .is_none()
        );
        let source = state
            .semantic_memory()
            .get_source(&context, old_card.source_id)
            .await
            .unwrap()
            .unwrap();
        assert!(source.pending_rebuild);
    }
}

#[tokio::test]
async fn local_rebind_fails_closed_after_rules_or_authorization_change() {
    for change in ["rules", "authorization"] {
        let (_dir, state, context, core, service) = fixture().await;
        let path = VaultPath::parse(&format!("notes/rebind-{change}.md")).unwrap();
        core.create_bytes(
            &context,
            &path,
            b"# Stable context\nKeep the bounded decision.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        let (old_card, _) = publish_decision(
            &service,
            &context,
            &core,
            &path,
            "Keep the bounded decision.",
            "Fence rebind",
        )
        .await;
        if change == "rules" {
            let target = state
                .semantic_rules()
                .list_targets(&context)
                .await
                .unwrap()
                .into_iter()
                .find(|target| target.target_kind == "memory_card")
                .unwrap();
            state
                .semantic_rules()
                .apply_suppression(
                    &context,
                    "memory_card",
                    "project",
                    target.fingerprint_version,
                    &target.fingerprint,
                    &json!({"reason":"rebind-rule-fence"}),
                    "suppress_read",
                    None,
                    None,
                    None,
                    Some(&old_card.id.to_string()),
                    None,
                    None,
                    "test-actor",
                    "rebind-rule-fence",
                )
                .await
                .unwrap();
        } else {
            let policy = state
                .settings()
                .get_vault(&context, "memory.units.policy")
                .await
                .unwrap()
                .unwrap();
            state
                .settings()
                .set_vault(
                    &context,
                    "memory.units.policy",
                    &json!({"enabled":true,"request_timeout_seconds":300}),
                    WritePrecondition::ExactRevision(policy.revision),
                    None,
                )
                .await
                .unwrap();
        }
        let current = core.read(&context, &path).await.unwrap().file;
        let changed = core
            .replace_bytes(
                &context,
                &path,
                current.current_revision,
                b"# Stable context\nAn unrelated paragraph changed.\nKeep the bounded decision.\n",
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await
            .unwrap();
        let report = service
            .reconcile_source_event(&context, &core, changed.file.id)
            .await
            .unwrap();
        assert_eq!(
            report.disposition,
            mcp_vault_memory::SemanticSourceEventDisposition::NeedsRebuild,
            "{change} fence must prevent a local rebind"
        );
        assert!(
            service
                .list_cards(&context, &core, 20)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            state
                .semantic_memory()
                .get_source(&context, old_card.source_id)
                .await
                .unwrap()
                .unwrap()
                .pending_rebuild
        );
    }
}

#[tokio::test]
async fn local_rebind_fails_closed_when_block_role_changes() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/rebind-role.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Context\n```\nKeep this exact line.\n```\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &path)
        .await
        .unwrap();
    let body = input
        .blocks
        .iter()
        .find(|block| block.text == "Keep this exact line.")
        .unwrap();
    let proposal = json!({"outcome":"success_nonempty","observations":[{"kind":"decision","statement":"Keep this exact line.","scope":"project","assertion_status":"source_asserted","admission_reason":"role test","value_for_future_work":"preserve role","body_block_ids":[body.local_id.clone()]}],"cards":[{"title":"Role card","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]});
    service
        .submit_proposal_json(&context, &core, &path, &proposal.to_string())
        .await
        .unwrap();
    let current = core.read(&context, &path).await.unwrap().file;
    let changed = core
        .replace_bytes(
            &context,
            &path,
            current.current_revision,
            b"# Context\nKeep this exact line.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let report = service
        .reconcile_source_event(&context, &core, changed.file.id)
        .await
        .unwrap();
    assert_eq!(
        report.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::NeedsRebuild
    );
    assert!(
        service
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        state
            .semantic_memory()
            .get_source_by_file(&context, changed.file.id)
            .await
            .unwrap()
            .unwrap()
            .pending_rebuild
    );
}

#[tokio::test]
async fn source_time_evidence_may_share_the_body_block_without_context_duplication() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/time-body.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"The decision is bounded.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &path)
        .await
        .unwrap();
    let body = input.blocks.first().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{
            "kind":"decision",
            "statement":"The decision is bounded.",
            "scope":"project",
            "assertion_status":"source_asserted",
            "source_time_scope":{"status":"source_stated","value":"2025","evidence_block_ids":[body.local_id.clone()]},
            "admission_reason":"time test",
            "value_for_future_work":"preserve the time scope",
            "body_block_ids":[body.local_id.clone()]
        }],
        "cards":[{"title":"Time card","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    let submission = service
        .submit_proposal_json(&context, &core, &path, &proposal.to_string())
        .await
        .unwrap();
    assert_eq!(submission.extraction.state, "success_nonempty");
    let card = service
        .list_cards(&context, &core, 20)
        .await
        .unwrap()
        .remove(0);
    let evidence_id = card.items[0].evidence_ref_ids[0];
    let evidence = state
        .semantic_memory()
        .get_evidence(&context, evidence_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(evidence.body_spans.len(), 1);
    assert!(evidence.context_spans.is_empty());
}

#[tokio::test]
async fn explicit_context_overlap_still_fails_closed() {
    let (_dir, _state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/overlap.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"The decision is bounded.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &path)
        .await
        .unwrap();
    let body = input.blocks.first().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{
            "kind":"decision",
            "statement":"The decision is bounded.",
            "scope":"project",
            "assertion_status":"source_asserted",
            "admission_reason":"overlap test",
            "value_for_future_work":"preserve the decision",
            "body_block_ids":[body.local_id.clone()],
            "context_block_ids":[body.local_id.clone()]
        }],
        "cards":[{"title":"Overlap","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    let error = service
        .submit_proposal_json(&context, &core, &path, &proposal.to_string())
        .await
        .unwrap_err();
    assert!(format!("{error:?}").contains("semantic_evidence_role_overlap"));
}

#[tokio::test]
async fn local_rebind_fails_closed_for_source_stated_time_scope() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/rebind-time.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Context\nThe decision is bounded.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &path)
        .await
        .unwrap();
    let heading = input
        .blocks
        .iter()
        .find(|block| block.text == "# Context")
        .unwrap();
    let body = input
        .blocks
        .iter()
        .find(|block| block.text == "The decision is bounded.")
        .unwrap();
    let proposal = json!({"outcome":"success_nonempty","observations":[{"kind":"decision","statement":"The decision is bounded.","scope":"project","assertion_status":"source_asserted","source_time_scope":{"status":"source_stated","value":"2025","evidence_block_ids":[heading.local_id.clone()]},"admission_reason":"time test","value_for_future_work":"preserve time scope","body_block_ids":[body.local_id.clone()]}],"cards":[{"title":"Time card","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]});
    service
        .submit_proposal_json(&context, &core, &path, &proposal.to_string())
        .await
        .unwrap();
    let current = core.read(&context, &path).await.unwrap().file;
    let changed = core
        .replace_bytes(
            &context,
            &path,
            current.current_revision,
            b"# Context\nAn unrelated paragraph.\nThe decision is bounded.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let report = service
        .reconcile_source_event(&context, &core, changed.file.id)
        .await
        .unwrap();
    assert_eq!(
        report.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::NeedsRebuild
    );
    assert!(
        service
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        state
            .semantic_memory()
            .get_source_by_file(&context, changed.file.id)
            .await
            .unwrap()
            .unwrap()
            .pending_rebuild
    );
}

#[tokio::test]
async fn local_rebind_rejects_corrupt_historical_observation_scope() {
    let (_dir, state, context, core, service, url) = file_backed_fixture().await;
    let path = VaultPath::parse("notes/rebind-corrupt.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Context\nKeep the historical decision.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let (old_card, _) = publish_decision(
        &service,
        &context,
        &core,
        &path,
        "Keep the historical decision.",
        "Corruption guard",
    )
    .await;
    let first_revision = state
        .semantic_memory()
        .get_source(&context, old_card.source_id)
        .await
        .unwrap()
        .unwrap();
    let first_revision_id = first_revision.current_revision_id.unwrap();
    let current = core.read(&context, &path).await.unwrap().file;
    core.replace_bytes(
        &context,
        &path,
        current.current_revision,
        b"# Context\nAn unrelated paragraph.\nKeep the historical decision.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let (_current_card, _) = publish_decision(
        &service,
        &context,
        &core,
        &path,
        "Keep the historical decision.",
        "Corruption guard current",
    )
    .await;
    let historical = state
        .semantic_memory()
        .latest_reusable_extraction(
            &context,
            old_card.source_id,
            state
                .semantic_memory()
                .get_source(&context, old_card.source_id)
                .await
                .unwrap()
                .unwrap()
                .current_revision_id
                .unwrap(),
            "semantic-memory-m1-v1",
        )
        .await
        .unwrap()
        .unwrap();
    let pool = SqlitePool::connect(&url).await.unwrap();
    sqlx::query(
        "UPDATE semantic_observations SET source_revision_id=?
         WHERE vault_id=? AND extraction_set_id=?",
    )
    .bind(first_revision_id.to_string())
    .bind(context.id().to_string())
    .bind(historical.id.to_string())
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    let current = core.read(&context, &path).await.unwrap().file;
    let changed = core
        .replace_bytes(
            &context,
            &path,
            current.current_revision,
            b"# Context\nAn unrelated paragraph.\nAnother unrelated paragraph.\nKeep the historical decision.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let report = service
        .reconcile_source_event(&context, &core, changed.file.id)
        .await
        .unwrap();
    assert_eq!(
        report.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::NeedsRebuild
    );
    assert!(
        service
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        state
            .semantic_memory()
            .get_source(&context, old_card.source_id)
            .await
            .unwrap()
            .unwrap()
            .pending_rebuild
    );
}

#[tokio::test]
async fn local_rebind_rejects_corrupt_historical_card_profile() {
    let (_dir, state, context, core, service, url) = file_backed_fixture().await;
    let path = VaultPath::parse("notes/rebind-profile-corrupt.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Context\nKeep the profile-fenced decision.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let (old_card, _) = publish_decision(
        &service,
        &context,
        &core,
        &path,
        "Keep the profile-fenced decision.",
        "Profile corruption guard",
    )
    .await;
    let source = state
        .semantic_memory()
        .get_source(&context, old_card.source_id)
        .await
        .unwrap()
        .unwrap();
    let extraction = state
        .semantic_memory()
        .latest_reusable_extraction(
            &context,
            old_card.source_id,
            source.current_revision_id.unwrap(),
            "semantic-memory-m1-v1",
        )
        .await
        .unwrap()
        .unwrap();
    let pool = SqlitePool::connect(&url).await.unwrap();
    sqlx::query(
        "UPDATE semantic_card_revisions SET composition_profile_id=?
         WHERE vault_id=? AND extraction_set_id=?",
    )
    .bind("wrong-profile")
    .bind(context.id().to_string())
    .bind(extraction.id.to_string())
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    let current = core.read(&context, &path).await.unwrap().file;
    let changed = core
        .replace_bytes(
            &context,
            &path,
            current.current_revision,
            b"# Context\nAn unrelated paragraph.\nKeep the profile-fenced decision.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let report = service
        .reconcile_source_event(&context, &core, changed.file.id)
        .await
        .unwrap();
    assert_eq!(
        report.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::NeedsRebuild
    );
    assert!(
        service
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        state
            .semantic_memory()
            .get_source(&context, old_card.source_id)
            .await
            .unwrap()
            .unwrap()
            .pending_rebuild
    );
}

#[tokio::test]
async fn correction_and_suppression_survive_file_backed_reconnect_and_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("semantic-rules.sqlite3");
    let url = format!("sqlite://{}", database.display());
    let state = StateStore::connect_and_migrate(&url).await.unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("rules-reconnect").unwrap(),
        directory.path().join("vault"),
        Revision::ZERO,
    )
    .unwrap();
    let other = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("rules-reconnect-other").unwrap(),
        directory.path().join("other"),
        Revision::ZERO,
    )
    .unwrap();
    for vault in [&context, &other] {
        state
            .vaults()
            .insert(vault, vault.slug().as_str(), VaultStatus::Active)
            .await
            .unwrap();
        state
            .settings()
            .set_vault(
                vault,
                "memory.units.policy",
                &json!({"enabled":true,"request_timeout_seconds":300}),
                WritePrecondition::Unconditional,
                None,
            )
            .await
            .unwrap();
    }
    let history = directory.path().join("history");
    let core = VaultCore::new(
        state.clone(),
        history.clone(),
        VaultPathPolicy::default(),
        StorageOptions::default(),
        Default::default(),
    );
    let path = VaultPath::parse("notes/rules.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Rules\nPersisted decision.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let service = SemanticMemoryService::new(state.clone());
    let input = service
        .prepare_source(&context, &core, &path)
        .await
        .unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({"outcome":"success_nonempty","observations":[{"kind":"decision","statement":"Persisted decision.","scope":"project","assertion_status":"source_asserted","admission_reason":"rules persistence","value_for_future_work":"preserve the rule boundary","body_block_ids":[body.local_id.clone()]}],"cards":[{"title":"Persisted rule card","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]});
    service
        .submit_proposal_json(&context, &core, &path, &proposal.to_string())
        .await
        .unwrap();
    let card = service
        .list_cards(&context, &core, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let target = state
        .semantic_rules()
        .list_targets(&context)
        .await
        .unwrap()
        .into_iter()
        .find(|target| {
            target.target_kind == "memory_card"
                && target.fingerprint.contains("Persisted decision.")
        })
        .unwrap();
    let rules = state.semantic_rules();
    rules
        .apply_suppression(
            &context,
            "memory_card",
            "project",
            target.fingerprint_version,
            &target.fingerprint,
            &json!({"reason":"persisted-forget"}),
            "forget_current",
            None,
            None,
            None,
            Some(&card.id.to_string()),
            None,
            None,
            "test-actor",
            "persisted-forget",
        )
        .await
        .unwrap();
    rules
        .apply_correction(
            &context,
            "memory_card",
            "project",
            target.fingerprint_version,
            &target.fingerprint,
            &json!({"replace":"Corrected decision.","remove":"Persisted decision."}),
            None,
            None,
            None,
            Some(&card.id.to_string()),
            None,
            None,
            "test-actor",
            "persisted-correction",
        )
        .await
        .unwrap();
    assert!(
        service
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(rules.current_rules_revision(&context).await.unwrap(), 2);

    state.close().await;
    let reopened = StateStore::connect_and_migrate(&url).await.unwrap();
    let reopened_core = VaultCore::new(
        reopened.clone(),
        history.clone(),
        VaultPathPolicy::default(),
        StorageOptions::default(),
        Default::default(),
    );
    let reopened_service = SemanticMemoryService::new(reopened.clone());
    let reopened_input = reopened_service
        .prepare_source(&context, &reopened_core, &path)
        .await
        .unwrap();
    let old_body = reopened_input.blocks.last().unwrap();
    let old_proposal = json!({"outcome":"success_nonempty","observations":[{"kind":"decision","statement":"Persisted decision.","scope":"project","assertion_status":"source_asserted","admission_reason":"replay old output","value_for_future_work":"old output must stay blocked","body_block_ids":[old_body.local_id.clone()]}],"cards":[{"title":"Persisted rule card","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]});
    assert!(
        reopened_service
            .submit_proposal_json(&context, &reopened_core, &path, &old_proposal.to_string())
            .await
            .is_err()
    );
    assert!(
        reopened_service
            .list_cards(&context, &reopened_core, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        reopened
            .semantic_rules()
            .current_rules_revision(&context)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        reopened
            .semantic_rules()
            .current_rules_revision(&other)
            .await
            .unwrap(),
        0
    );

    let source = reopened
        .semantic_memory()
        .get_source_by_file(
            &context,
            reopened_core.read(&context, &path).await.unwrap().file.id,
        )
        .await
        .unwrap()
        .unwrap();
    let cancelled = ExtractionSetId::new();
    reopened
        .semantic_memory()
        .start_extraction(
            &context,
            cancelled,
            source.source_id,
            source.current_revision_id.unwrap(),
            &source.content_hash,
            "semantic-memory-m1-v1",
            "rules-recovery-cancel",
            "rules-recovery-cancel-request",
        )
        .await
        .unwrap();
    reopened_service
        .cancel_extraction(&context, cancelled)
        .await
        .unwrap();
    reopened.close().await;
    let recovered = StateStore::connect_and_migrate(&url).await.unwrap();
    let recovered_core = VaultCore::new(
        recovered.clone(),
        history,
        VaultPathPolicy::default(),
        StorageOptions::default(),
        Default::default(),
    );
    SemanticMemoryService::new(recovered.clone())
        .recover_pending_publications(&context, &recovered_core)
        .await
        .unwrap();
    assert!(
        SemanticMemoryService::new(recovered.clone())
            .list_cards(&context, &recovered_core, 20)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn single_source_card_round_trip_preserves_conditions_and_evidence() {
    let (_dir, state, context, core, service) = fixture().await;
    let source_path = VaultPath::parse("notes/plan.md").unwrap();
    core.create_bytes(
        &context,
        &source_path,
        b"# Release plan\nThe migration must preserve rollback order.\nIt must not delete ordinary notes.\nException: production changes require approval.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    ).await.unwrap();
    let input = service
        .prepare_source(&context, &core, &source_path)
        .await
        .unwrap();
    let body = input
        .blocks
        .iter()
        .find(|block| block.text.contains("must preserve"))
        .unwrap();
    let exception = input
        .blocks
        .iter()
        .find(|block| block.text.contains("Exception"))
        .unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{
            "kind":"constraint",
            "statement":"The migration must preserve rollback order and must not delete ordinary notes.",
            "scope":"project",
            "assertion_status":"source_asserted",
            "conditions":["Rollback order remains part of the migration procedure."],
            "exceptions":["Production changes require approval."],
            "ordered_steps":["Preserve rollback order", "Avoid deletion of ordinary notes"],
            "result":"The ordinary Vault remains portable.",
            "admission_reason":"It constrains future migration work.",
            "value_for_future_work":"Use this constraint when reviewing implementation changes.",
            "body_block_ids":[body.local_id.clone()],
            "context_block_ids":[exception.local_id.clone()]
        }],
        "cards":[{
            "title":"Migration safety",
            "kind":"constraint",
            "scope":"project",
            "assertion_status":"source_asserted",
            "observation_indices":[0]
        }]
    });
    let submission = service
        .submit_proposal_json(&context, &core, &source_path, &proposal.to_string())
        .await
        .unwrap_or_else(|error| panic!("submission failed: {error:?}"));
    assert_eq!(submission.extraction.state, "success_nonempty");
    let cards = service.list_cards(&context, &core, 20).await.unwrap();
    assert_eq!(cards.len(), 1);
    let card = &cards[0];
    assert!(
        card.items
            .iter()
            .any(|item| item.content.contains("must not delete"))
    );
    assert!(card.items.iter().any(|item| item.kind == "exception"));
    assert!(card.items.iter().any(|item| item.kind == "ordered_step"));
    let evidence_id = card.items[0].evidence_ref_ids[0];
    let evidence = service
        .read_evidence(&context, &core, evidence_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        evidence
            .body_spans
            .iter()
            .any(|span| span.contains("preserve rollback"))
    );
    assert!(
        evidence
            .context_spans
            .iter()
            .any(|span| span.contains("production changes"))
    );
    let extraction_id = submission.extraction.id;
    let persisted = state
        .semantic_memory()
        .get_extraction(&context, extraction_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.observation_count, 1);
    assert_eq!(persisted.card_count, 1);
}

#[tokio::test]
async fn core_file_commits_hide_semantic_cards_and_evidence_before_outbox_consumption() {
    let (_dir, state, context, core, service) = fixture().await;
    let source_path = VaultPath::parse("notes/qualification.md").unwrap();
    core.create_bytes(
        &context,
        &source_path,
        b"# Qualification\nKeep the current source boundary.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &source_path)
        .await
        .unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":"Keep the current source boundary.","scope":"project","assertion_status":"source_asserted","admission_reason":"boundary","value_for_future_work":"preserve the boundary","body_block_ids":[body.local_id.clone()]}],
        "cards":[{"title":"Source boundary","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    service
        .submit_proposal_json(&context, &core, &source_path, &proposal.to_string())
        .await
        .unwrap();
    let card = service
        .list_cards(&context, &core, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let evidence_id = card.items[0].evidence_ref_ids[0];
    let before_revision = core
        .read(&context, &source_path)
        .await
        .unwrap()
        .file
        .current_revision;

    // The State qualification runs in Core's metadata transaction. No outbox
    // worker is involved before these reads.
    core.replace_bytes(
        &context,
        &source_path,
        before_revision,
        b"# Qualification\nThe source boundary changed.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    assert!(
        service
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        service
            .read_evidence(&context, &core, evidence_id)
            .await
            .unwrap()
            .is_none()
    );

    // Re-publish the changed revision, then verify delete has the same
    // synchronous fail-closed behavior.
    let input = service
        .prepare_source(&context, &core, &source_path)
        .await
        .unwrap();
    let body = input.blocks.last().unwrap();
    let changed = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":"The source boundary changed.","scope":"project","assertion_status":"source_asserted","admission_reason":"boundary","value_for_future_work":"reconfirm the boundary","body_block_ids":[body.local_id.clone()]}],
        "cards":[{"title":"Source boundary","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    service
        .submit_proposal_json(&context, &core, &source_path, &changed.to_string())
        .await
        .unwrap();
    let current = core.read(&context, &source_path).await.unwrap().file;
    core.delete(
        &context,
        &source_path,
        current.current_revision,
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    assert!(
        state
            .semantic_memory()
            .list_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn restore_commit_hides_semantic_cards_and_evidence_immediately() {
    let (_dir, state, context, core, service) = fixture().await;
    let source_path = VaultPath::parse("notes/restore.md").unwrap();
    core.create_bytes(
        &context,
        &source_path,
        b"# Restore\nThe original source is restorable.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &source_path)
        .await
        .unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":"The original source is restorable.","scope":"project","assertion_status":"source_asserted","admission_reason":"restore test","value_for_future_work":"preserve restore evidence","body_block_ids":[body.local_id.clone()]}],
        "cards":[{"title":"Restorable source","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    service
        .submit_proposal_json(&context, &core, &source_path, &proposal.to_string())
        .await
        .unwrap();
    let changed = core
        .replace_bytes(
            &context,
            &source_path,
            Revision::new(1),
            b"# Restore\nThe source was changed before restore.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let input = service
        .prepare_source(&context, &core, &source_path)
        .await
        .unwrap();
    let body = input.blocks.last().unwrap();
    let changed_proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":"The source was changed before restore.","scope":"project","assertion_status":"source_asserted","admission_reason":"restore test current revision","value_for_future_work":"preserve the current evidence","body_block_ids":[body.local_id.clone()]}],
        "cards":[{"title":"Current restorable source","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    service
        .submit_proposal_json(&context, &core, &source_path, &changed_proposal.to_string())
        .await
        .unwrap();
    let changed_card = service
        .list_cards(&context, &core, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let changed_evidence_id = changed_card.items[0].evidence_ref_ids[0];
    assert!(
        changed_card.items[0]
            .content
            .contains("source was changed before restore")
    );
    assert!(
        service
            .read_evidence(&context, &core, changed_evidence_id)
            .await
            .unwrap()
            .is_some()
    );
    let restored = core
        .restore(
            &context,
            &source_path,
            Revision::new(1),
            changed.file.current_revision,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    assert_eq!(restored.file.current_revision, Revision::new(3));
    assert!(
        service
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        service
            .read_evidence(&context, &core, changed_evidence_id)
            .await
            .unwrap()
            .is_none()
    );
    let source = state
        .semantic_memory()
        .get_source_by_file(&context, restored.file.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!source.eligible);
    assert!(source.pending_rebuild);
}

#[tokio::test]
async fn failed_file_commit_rolls_back_semantic_invalidation() {
    let (_dir, state, context, core, service) = fixture().await;
    let source_path = VaultPath::parse("notes/rollback.md").unwrap();
    core.create_bytes(
        &context,
        &source_path,
        b"# Rollback\nKeep the source card readable.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &source_path)
        .await
        .unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":"Keep the source card readable.","scope":"project","assertion_status":"source_asserted","admission_reason":"rollback test","value_for_future_work":"retain current evidence","body_block_ids":[body.local_id.clone()]}],
        "cards":[{"title":"Rollback source","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    service
        .submit_proposal_json(&context, &core, &source_path, &proposal.to_string())
        .await
        .unwrap();
    let before = service
        .list_cards(&context, &core, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let source_before = state
        .semantic_memory()
        .get_source(&context, before.source_id)
        .await
        .unwrap()
        .unwrap();
    let failing_core = core.clone().with_failure_injector(Arc::new(FailOnce {
        phase: CommitPhase::OutboxInserted,
        fired: AtomicBool::new(false),
    }));
    assert!(
        failing_core
            .replace_bytes(
                &context,
                &source_path,
                Revision::new(1),
                b"# Rollback\nThis mutation will roll back metadata.\n",
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await
            .is_err()
    );
    let after = state
        .semantic_memory()
        .get_source(&context, before.source_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.source_generation, source_before.source_generation);
    assert!(after.eligible);
    assert!(!after.pending_rebuild);
    assert_eq!(
        service.list_cards(&context, &core, 20).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn policy_revision_invalidates_semantic_reads_before_return_and_stays_vault_scoped() {
    let (_dir, state, context, core, service) = fixture().await;
    let source_path = VaultPath::parse("notes/policy.md").unwrap();
    core.create_bytes(
        &context,
        &source_path,
        b"# Policy\nKeep this policy boundary.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &source_path)
        .await
        .unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"constraint","statement":"Keep this policy boundary.","scope":"project","assertion_status":"source_asserted","admission_reason":"policy","value_for_future_work":"preserve policy","body_block_ids":[body.local_id.clone()]}],
        "cards":[{"title":"Policy boundary","kind":"constraint","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    service
        .submit_proposal_json(&context, &core, &source_path, &proposal.to_string())
        .await
        .unwrap();
    assert_eq!(
        service.list_cards(&context, &core, 20).await.unwrap().len(),
        1
    );
    let revision = state
        .settings()
        .get_vault(&context, "memory.units.policy")
        .await
        .unwrap()
        .unwrap()
        .revision;
    state
        .settings()
        .set_vault(
            &context,
            "memory.units.policy",
            &json!({"enabled":true,"request_timeout_seconds":301}),
            WritePrecondition::ExactRevision(revision),
            None,
        )
        .await
        .unwrap();
    assert!(
        service
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn forged_block_and_unknown_fields_are_rejected_without_empty_publish() {
    let (_dir, _state, context, core, service) = fixture().await;
    let source_path = VaultPath::parse("notes/plan.md").unwrap();
    core.create_bytes(
        &context,
        &source_path,
        b"# Plan\nKeep the exception.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &source_path)
        .await
        .unwrap();
    let forged = json!({"outcome":"success_nonempty","observations":[{
        "kind":"constraint","statement":"Keep the exception.","scope":"project","assertion_status":"source_asserted",
        "admission_reason":"reason","value_for_future_work":"value","body_block_ids":["forged"],"unknown":true
    }],"cards":[]});
    assert!(
        service
            .submit_proposal_json(&context, &core, &source_path, &forged.to_string())
            .await
            .is_err()
    );
    let status = service
        .prepare_source(&context, &core, &source_path)
        .await
        .unwrap();
    assert_eq!(status.blocks.len(), input.blocks.len());
    let body = status.blocks.last().unwrap();
    let mismatched_kind = json!({"outcome":"success_nonempty","observations":[{
        "kind":"constraint","statement":"Keep the exception.","scope":"project","assertion_status":"source_asserted",
        "admission_reason":"reason","value_for_future_work":"value","body_block_ids":[body.local_id.clone()]
    }],"cards":[{"title":"Wrong kind","kind":"experience","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]});
    assert!(
        service
            .submit_proposal_json(&context, &core, &source_path, &mismatched_kind.to_string())
            .await
            .is_err()
    );
    let missing_outcome = json!({"observations":[],"cards":[]});
    assert!(
        service
            .submit_proposal_json(&context, &core, &source_path, &missing_outcome.to_string())
            .await
            .is_err(),
        "legacy complete proposal input continues to require outcome"
    );
}

#[tokio::test]
async fn two_stage_generation_fences_and_derives_multi_group_cards() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/two-stage.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Two stage\nConstraint one.\nState one.\nConstraint two.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let prepared = service
        .begin_generation(&context, &core, &path)
        .await
        .unwrap();
    let blocks = prepared.model_input.blocks.clone();
    let body = |index: usize| blocks[index].local_id.clone();
    let observations = json!({
        "observations":[
            {"kind":"constraint","statement":"Constraint one.","scope":"project","assertion_status":"source_asserted","admission_reason":"source","value_for_future_work":"keep","body_block_ids":[body(1)]},
            {"kind":"state","statement":"State one.","scope":"project","assertion_status":"source_asserted","admission_reason":"source","value_for_future_work":"keep","body_block_ids":[body(2)]},
            {"kind":"constraint","statement":"Constraint two.","scope":"project","assertion_status":"source_asserted","admission_reason":"source","value_for_future_work":"keep","body_block_ids":[body(3)]}
        ]
    });
    let phase = service
        .accept_observation_result(&context, &core, prepared, &observations.to_string())
        .await
        .unwrap();
    assert_eq!(phase.groups.len(), 2);
    let composition = json!({"cards":[
        {"title":"Constraint one","observation_indices":[0]},
        {"title":"Constraint two","observation_indices":[2]},
        {"title":"State one","observation_indices":[1]}
    ]});
    let submission = service
        .submit_composition(&context, &core, &path, &phase, &composition.to_string())
        .await
        .unwrap();
    assert_eq!(submission.extraction.state, "success_nonempty");
    let cards = service.list_cards(&context, &core, 20).await.unwrap();
    assert_eq!(cards.len(), 3);
    assert!(cards.iter().any(|card| card.kind == "constraint"));
    assert!(cards.iter().any(|card| card.kind == "state"));
    assert_eq!(
        state
            .semantic_memory()
            .list_cards(&context, 20)
            .await
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn two_stage_cross_group_selection_fails_without_partial_publication() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/two-stage-fail.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Fail\nConstraint.\nState.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let prepared = service
        .begin_generation(&context, &core, &path)
        .await
        .unwrap();
    let blocks = prepared.model_input.blocks.clone();
    let observations = json!({"outcome":"success_nonempty","observations":[
        {"kind":"constraint","statement":"Constraint.","scope":"project","assertion_status":"source_asserted","admission_reason":"source","value_for_future_work":"keep","body_block_ids":[blocks[1].local_id.clone()]},
        {"kind":"state","statement":"State.","scope":"project","assertion_status":"source_asserted","admission_reason":"source","value_for_future_work":"keep","body_block_ids":[blocks[2].local_id.clone()]}
    ]});
    let phase = service
        .accept_observation_result(&context, &core, prepared, &observations.to_string())
        .await
        .unwrap();
    let bad = json!({"cards":[{"title":"mixed","observation_indices":[0,1]}]});
    assert!(
        service
            .submit_composition(&context, &core, &path, &phase, &bad.to_string())
            .await
            .is_err()
    );
    assert!(
        state
            .semantic_memory()
            .list_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        state
            .semantic_memory()
            .get_extraction(&context, phase.extraction.id)
            .await
            .unwrap()
            .unwrap()
            .state
            .as_str(),
        "failed" | "cancelled"
    ));
}

#[tokio::test]
async fn two_stage_source_revision_change_is_rejected_before_observation_acceptance() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/two-stage-revision.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Revision\nOriginal.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let prepared = service
        .begin_generation(&context, &core, &path)
        .await
        .unwrap();
    let extraction_id = prepared.extraction.id;
    let file = core.read(&context, &path).await.unwrap().file;
    core.replace_bytes(
        &context,
        &path,
        file.current_revision,
        b"# Revision\nChanged.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let body = prepared.model_input.blocks.last().unwrap().local_id.clone();
    let observations = json!({"outcome":"success_nonempty","observations":[{"kind":"state","statement":"Original.","scope":"project","assertion_status":"source_asserted","admission_reason":"source","value_for_future_work":"keep","body_block_ids":[body]}]});
    assert!(
        service
            .accept_observation_result(&context, &core, prepared, &observations.to_string())
            .await
            .is_err()
    );
    assert!(
        state
            .semantic_memory()
            .list_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
    let extraction = state
        .semantic_memory()
        .get_extraction(&context, extraction_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(extraction.state.as_str(), "failed" | "cancelled"));
}

#[tokio::test]
async fn two_stage_time_evidence_resolves_revision_bound_ids_from_prepared_input() {
    let (_dir, _state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/two-stage-time-evidence.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"The decision is effective in 2025.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let prepared = service
        .begin_generation(&context, &core, &path)
        .await
        .unwrap();
    let evidence_id = prepared.model_input.blocks[0].local_id.clone();
    let observation = json!({
        "observations":[{
            "kind":"decision",
            "statement":"The decision is effective in 2025.",
            "scope":"project",
            "assertion_status":"source_asserted",
            "source_time_scope":{
                "status":"source_stated",
                "value":"2025",
                "evidence_block_ids":[evidence_id.clone()]
            },
            "admission_reason":"prepared time evidence test",
            "value_for_future_work":"preserve the stated year",
            "body_block_ids":[evidence_id]
        }]
    });

    let phase = service
        .accept_observation_result(&context, &core, prepared, &observation.to_string())
        .await
        .unwrap();
    let time_scope = phase.observations[0].source_time_scope.as_ref().unwrap();
    assert_eq!(time_scope.value.as_deref(), Some("2025"));
    assert_eq!(time_scope.evidence_block_ids.len(), 1);
    assert_eq!(
        phase.observations[0].body_block_ids,
        time_scope.evidence_block_ids
    );
}

#[tokio::test]
async fn cross_source_observation_replay_is_rejected_for_different_and_matching_first_lines() {
    for (case, source_a, source_b) in [
        (
            "different-first-line",
            b"# Source A\nA detail.\n".as_slice(),
            b"# Source B\nB detail.\n".as_slice(),
        ),
        (
            "matching-first-line",
            b"# Shared heading\nA detail.\n".as_slice(),
            b"# Shared heading\nB detail.\n".as_slice(),
        ),
    ] {
        let (_dir, state, context, core, service) = fixture().await;
        let path_a = VaultPath::parse(&format!("notes/replay-a-{case}.md")).unwrap();
        let path_b = VaultPath::parse(&format!("notes/replay-b-{case}.md")).unwrap();
        core.create_bytes(
            &context,
            &path_a,
            source_a,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        core.create_bytes(
            &context,
            &path_b,
            source_b,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();

        let prepared_a = service
            .begin_generation(&context, &core, &path_a)
            .await
            .unwrap();
        let prepared_b = service
            .begin_generation(&context, &core, &path_b)
            .await
            .unwrap();
        let source_a_id = prepared_a.model_input.blocks[0].local_id.clone();
        let source_b_id = prepared_b.model_input.blocks[0].local_id.clone();
        assert_ne!(source_a_id, source_b_id);
        let extraction_b = prepared_b.extraction.id;
        let stale_a_response = json!({
            "observations":[{
                "kind":"state",
                "statement":"A response must not bind to B.",
                "scope":"project",
                "assertion_status":"source_asserted",
                "admission_reason":"cross-source replay test",
                "value_for_future_work":"reject stale source evidence",
                "body_block_ids":[source_a_id]
            }]
        });

        assert!(
            service
                .accept_observation_result(
                    &context,
                    &core,
                    prepared_b,
                    &stale_a_response.to_string()
                )
                .await
                .is_err()
        );
        assert!(
            state
                .semantic_memory()
                .list_cards(&context, 20)
                .await
                .unwrap()
                .is_empty()
        );
        let extraction = state
            .semantic_memory()
            .get_extraction(&context, extraction_b)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(extraction.state.as_str(), "failed" | "cancelled"));
    }
}

#[tokio::test]
async fn two_stage_external_source_change_without_state_sync_is_rejected() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/external.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# External\nOriginal.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let prepared = service
        .begin_generation(&context, &core, &path)
        .await
        .unwrap();
    let extraction_id = prepared.extraction.id;
    fs::write(
        context.content_root().join("notes/external.md"),
        b"# External\nChanged outside Core.\n",
    )
    .unwrap();
    let body = prepared.model_input.blocks.last().unwrap().local_id.clone();
    let observations = json!({"outcome":"success_nonempty","observations":[{"kind":"state","statement":"Original.","scope":"project","assertion_status":"source_asserted","admission_reason":"source","value_for_future_work":"keep","body_block_ids":[body]}]});
    assert!(
        service
            .accept_observation_result(&context, &core, prepared, &observations.to_string())
            .await
            .is_err()
    );
    assert!(
        state
            .semantic_memory()
            .list_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
    let extraction = state
        .semantic_memory()
        .get_extraction(&context, extraction_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(extraction.state.as_str(), "failed" | "cancelled"));
}

#[tokio::test]
async fn two_stage_source_change_between_phases_blocks_publication() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/between.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Between\nOriginal.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let prepared = service
        .begin_generation(&context, &core, &path)
        .await
        .unwrap();
    let body = prepared.model_input.blocks.last().unwrap().local_id.clone();
    let observations = json!({"outcome":"success_nonempty","observations":[{"kind":"state","statement":"Original.","scope":"project","assertion_status":"source_asserted","admission_reason":"source","value_for_future_work":"keep","body_block_ids":[body]}]});
    let phase = service
        .accept_observation_result(&context, &core, prepared, &observations.to_string())
        .await
        .unwrap();
    let file = core.read(&context, &path).await.unwrap().file;
    core.replace_bytes(
        &context,
        &path,
        file.current_revision,
        b"# Between\nChanged.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let composition = json!({"cards":[{"title":"state","observation_indices":[0]}]});
    assert!(
        service
            .submit_composition(&context, &core, &path, &phase, &composition.to_string())
            .await
            .is_err()
    );
    assert!(
        state
            .semantic_memory()
            .list_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        state
            .semantic_memory()
            .get_extraction(&context, phase.extraction.id)
            .await
            .unwrap()
            .unwrap()
            .state
            .as_str(),
        "failed" | "cancelled"
    ));
}

#[tokio::test]
async fn two_stage_empty_completes_without_composition() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/two-stage-empty.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Empty\nNo durable memory.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let prepared = service
        .begin_generation(&context, &core, &path)
        .await
        .unwrap();
    let phase = service
        .accept_observation_result(&context, &core, prepared, r#"{"observations":[]}"#)
        .await
        .unwrap();
    assert!(phase.observations.is_empty());
    assert_eq!(
        state
            .semantic_memory()
            .get_extraction(&context, phase.extraction.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "success_empty"
    );
    assert!(
        service
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn observation_stage_rejects_mismatch_cards_unknown_keys_and_forged_evidence() {
    for (name, make_input) in [
        ("mismatch", 0),
        ("cards", 1),
        ("unknown", 2),
        ("forged-evidence", 3),
        ("null-outcome", 4),
        ("duplicate-evidence", 5),
    ] {
        let (_dir, state, context, core, service) = fixture().await;
        let path = VaultPath::parse(&format!("notes/observation-reject-{name}.md")).unwrap();
        core.create_bytes(
            &context,
            &path,
            b"# Observation rejection\nCurrent fact.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        let prepared = service
            .begin_generation(&context, &core, &path)
            .await
            .unwrap();
        let extraction_id = prepared.extraction.id;
        let body = prepared.model_input.blocks.last().unwrap().local_id.clone();
        let mut input = json!({"observations":[{
            "kind":"state","statement":"Current fact.","scope":"project",
            "assertion_status":"source_asserted","admission_reason":"source",
            "value_for_future_work":"retain","body_block_ids":[body]
        }]});
        match make_input {
            0 => input["outcome"] = json!("success_empty"),
            1 => input["cards"] = json!([]),
            2 => input["debug"] = json!(true),
            3 => input["observations"][0]["body_block_ids"] = json!(["forged-evidence-id"]),
            4 => input["outcome"] = json!(null),
            5 => {
                let repeated_id = input["observations"][0]["body_block_ids"][0].clone();
                input["observations"][0]["body_block_ids"] =
                    json!([repeated_id.clone(), repeated_id]);
            }
            _ => unreachable!(),
        }
        assert!(
            service
                .accept_observation_result(&context, &core, prepared, &input.to_string())
                .await
                .is_err(),
            "observation input case {name} must be rejected"
        );
        assert!(
            state
                .semantic_memory()
                .list_cards(&context, 20)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            state
                .semantic_memory()
                .get_extraction(&context, extraction_id)
                .await
                .unwrap()
                .unwrap()
                .state
                .as_str(),
            "failed" | "cancelled"
        ));
    }
}

#[tokio::test]
async fn malformed_observation_json_fails_and_terminalizes_extraction() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/malformed-observation-json.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Malformed proposal\nKeep the extraction terminal.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let prepared = service
        .begin_generation(&context, &core, &path)
        .await
        .unwrap();
    let extraction_id = prepared.extraction.id;
    let result = service
        .accept_observation_result(&context, &core, prepared, r#"{"observations":["#)
        .await;
    let error = result.err().expect("malformed proposal must fail");
    assert_eq!(error.code(), "semantic_proposal_schema");
    let extraction = state
        .semantic_memory()
        .get_extraction(&context, extraction_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(extraction.state.as_str(), "failed" | "cancelled"));
    assert_eq!(
        extraction.safe_error_code.as_deref(),
        Some("semantic_proposal_schema")
    );
    assert!(
        state
            .semantic_memory()
            .list_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn two_stage_empty_rejects_contradictory_outcome_and_terminates() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/two-stage-empty-mismatch.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Empty\nNo durable memory.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let prepared = service
        .begin_generation(&context, &core, &path)
        .await
        .unwrap();
    let extraction_id = prepared.extraction.id;
    let result = service
        .accept_observation_result(
            &context,
            &core,
            prepared,
            r#"{"outcome":"success_nonempty","observations":[]}"#,
        )
        .await;
    assert!(result.is_err());
    let error = result.err().unwrap();
    assert_eq!(error.code(), "semantic_observation_outcome_mismatch");
    let extraction = state
        .semantic_memory()
        .get_extraction(&context, extraction_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(extraction.state.as_str(), "failed" | "cancelled"));
    assert_eq!(
        extraction.safe_error_code.as_deref(),
        Some("semantic_observation_outcome_mismatch")
    );
}

#[tokio::test]
async fn two_stage_policy_change_after_begin_is_rejected() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/two-stage-policy.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Policy\nState.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let prepared = service
        .begin_generation(&context, &core, &path)
        .await
        .unwrap();
    let extraction_id = prepared.extraction.id;
    state
        .settings()
        .set_vault(
            &context,
            "memory.units.policy",
            &json!({"enabled":false,"request_timeout_seconds":300}),
            WritePrecondition::Unconditional,
            None,
        )
        .await
        .unwrap();
    let body = prepared.model_input.blocks.last().unwrap().local_id.clone();
    let observations = json!({"outcome":"success_nonempty","observations":[{"kind":"state","statement":"State.","scope":"project","assertion_status":"source_asserted","admission_reason":"source","value_for_future_work":"keep","body_block_ids":[body]}]});
    assert!(
        service
            .accept_observation_result(&context, &core, prepared, &observations.to_string())
            .await
            .is_err()
    );
    assert!(matches!(
        state
            .semantic_memory()
            .get_extraction(&context, extraction_id)
            .await
            .unwrap()
            .unwrap()
            .state
            .as_str(),
        "failed" | "cancelled"
    ));
}

#[tokio::test]
async fn two_stage_prepared_generation_cannot_cross_vaults() {
    let (_dir_a, _state_a, context_a, core_a, service_a) = fixture().await;
    let (_dir_b, state_b, context_b, core_b, service_b) = fixture().await;
    let path = VaultPath::parse("notes/cross-vault.md").unwrap();
    core_a
        .create_bytes(
            &context_a,
            &path,
            b"# Cross vault\nState.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let prepared = service_a
        .begin_generation(&context_a, &core_a, &path)
        .await
        .unwrap();
    let body = prepared.model_input.blocks.last().unwrap().local_id.clone();
    let observations = json!({"outcome":"success_nonempty","observations":[{
        "kind":"state","statement":"State.","scope":"project","assertion_status":"source_asserted",
        "admission_reason":"source","value_for_future_work":"keep","body_block_ids":[body]
    }]});
    assert!(
        service_b
            .accept_observation_result(&context_b, &core_b, prepared, &observations.to_string())
            .await
            .is_err()
    );
    assert!(
        state_b
            .semantic_memory()
            .list_cards(&context_b, 20)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn two_stage_composition_duplicate_or_missing_refs_fails_without_publication() {
    for bad in [
        json!({"cards":[{"title":"duplicate","observation_indices":[0]},{"title":"again","observation_indices":[0]}]}),
        json!({"cards":[{"title":"missing","observation_indices":[0]}]}),
    ] {
        let (_dir, state, context, core, service) = fixture().await;
        let path = VaultPath::parse("notes/composition-refs.md").unwrap();
        core.create_bytes(
            &context,
            &path,
            b"# Refs\nFirst.\nSecond.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        let prepared = service
            .begin_generation(&context, &core, &path)
            .await
            .unwrap();
        let blocks = prepared.model_input.blocks.clone();
        let observations = json!({"outcome":"success_nonempty","observations":[
            {"kind":"state","statement":"First.","scope":"project","assertion_status":"source_asserted","admission_reason":"source","value_for_future_work":"keep","body_block_ids":[blocks[1].local_id.clone()]},
            {"kind":"state","statement":"Second.","scope":"project","assertion_status":"source_asserted","admission_reason":"source","value_for_future_work":"keep","body_block_ids":[blocks[2].local_id.clone()]}
        ]});
        let phase = service
            .accept_observation_result(&context, &core, prepared, &observations.to_string())
            .await
            .unwrap();
        assert!(
            service
                .submit_composition(&context, &core, &path, &phase, &bad.to_string())
                .await
                .is_err()
        );
        assert!(
            state
                .semantic_memory()
                .list_cards(&context, 20)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            state
                .semantic_memory()
                .get_extraction(&context, phase.extraction.id)
                .await
                .unwrap()
                .unwrap()
                .state,
            "failed"
        );
    }
}

#[tokio::test]
async fn two_stage_rules_revision_change_after_observation_rejects_without_publication() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/rules-fence.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Rules fence\nState.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let prepared = service
        .begin_generation(&context, &core, &path)
        .await
        .unwrap();
    let body = prepared.model_input.blocks.last().unwrap().local_id.clone();
    let observations = json!({"outcome":"success_nonempty","observations":[{
        "kind":"state","statement":"State.","scope":"project","assertion_status":"source_asserted",
        "admission_reason":"source","value_for_future_work":"keep","body_block_ids":[body]
    }]});
    let phase = service
        .accept_observation_result(&context, &core, prepared, &observations.to_string())
        .await
        .unwrap();
    let target = state
        .semantic_rules()
        .register_target(&context, "memory_card", "project", 1, "rules-fence-bump")
        .await
        .unwrap();
    state
        .semantic_rules()
        .apply_suppression(
            &context,
            "memory_card",
            "project",
            target.fingerprint_version,
            &target.fingerprint,
            &json!({"reason":"rules-fence-bump"}),
            "forget_current",
            None,
            None,
            None,
            None,
            None,
            None,
            "test-actor",
            "rules-fence-bump",
        )
        .await
        .unwrap();
    let composition = json!({"cards":[{"title":"State","observation_indices":[0]}]});
    assert!(
        service
            .submit_composition(&context, &core, &path, &phase, &composition.to_string())
            .await
            .is_err()
    );
    assert!(
        state
            .semantic_memory()
            .list_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        state
            .semantic_memory()
            .get_extraction(&context, phase.extraction.id)
            .await
            .unwrap()
            .unwrap()
            .state
            .as_str(),
        "failed" | "cancelled"
    ));
}

#[tokio::test]
async fn success_empty_is_distinct_from_failure_and_invalidates_old_cards() {
    let (_dir, state, context, core, service) = fixture().await;
    let source_path = VaultPath::parse("notes/empty.md").unwrap();
    core.create_bytes(
        &context,
        &source_path,
        b"# Empty\nNo durable memory.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &source_path)
        .await
        .unwrap();
    let body = input.blocks.last().unwrap();
    let first = json!({"outcome":"success_nonempty","observations":[{
        "kind":"state","statement":"No durable memory.","scope":"project","assertion_status":"source_asserted",
        "admission_reason":"state","value_for_future_work":"state","body_block_ids":[body.local_id.clone()]
    }],"cards":[{"title":"Initial state","kind":"state","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]});
    let published = service
        .submit_proposal_json(&context, &core, &source_path, &first.to_string())
        .await
        .unwrap_or_else(|error| panic!("first submission failed: {error:?}"));
    assert_eq!(published.extraction.state, "success_nonempty");
    let empty = json!({"outcome":"success_empty","observations":[]});
    let result = service
        .submit_proposal_json(&context, &core, &source_path, &empty.to_string())
        .await
        .unwrap();
    assert_eq!(result.extraction.state, "success_empty");
    assert!(
        service
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        state
            .semantic_memory()
            .list_cards(&context, 20)
            .await
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn a80_batches_validate_before_atomic_one_observation_cards_publish() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/a80-batches.md").unwrap();
    let mut content = String::from("# A80 batch source\n");
    for index in 0..85 {
        content.push_str(&format!("Supported source statement {index}.\n"));
    }
    core.create_bytes(
        &context,
        &path,
        content.as_bytes(),
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let prepared = service
        .begin_generation(&context, &core, &path)
        .await
        .unwrap();
    let extraction_id = prepared.extraction.id;
    let batches = prepared.observation_batches().unwrap();
    assert_eq!(batches.len(), 2);
    assert!(
        batches
            .iter()
            .all(|batch| { batch.input["blocks"].as_array().unwrap().len() <= 80 })
    );
    service
        .bind_observation_batches(
            &context,
            &prepared,
            &batches,
            "fake-provider-binding",
            "semantic-cards-tracked-adr-m6-v14",
            "semantic-cards-m6-json-v10",
        )
        .await
        .unwrap();
    for (batch_index, batch) in batches.iter().enumerate() {
        assert_eq!(
            service
                .reserve_observation_batch_attempt(&context, &prepared, batch)
                .await
                .unwrap(),
            "dispatching"
        );
        let first = batch.input["blocks"][0]["evidence_index"].as_u64().unwrap();
        let statement = format!(
            "Supported source statement {}.",
            if batch_index == 0 { 0 } else { 80 }
        );
        let response = json!({
            "claims":[{
                "statement":statement,
                "evidence_indices":[if batch_index == 0 { first + 1 } else { first }],
                "kind": if batch_index == 0 { "not_a_semantic_kind" } else { "decision" },
                "scope":"project",
                "assertion_status":"source_asserted"
            }]
        });
        service
            .accept_observation_batch(&context, &core, &prepared, batch, &response.to_string())
            .await
            .unwrap();
        assert!(
            state
                .semantic_memory()
                .list_cards(&context, 20)
                .await
                .unwrap()
                .is_empty()
        );
    }
    let published = service
        .finalize_observation_batches(
            &context,
            &core,
            &path,
            &prepared,
            u32::try_from(batches.len()).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(published.extraction.state, "success_nonempty");
    assert_eq!(published.extraction.observation_count, 2);
    assert_eq!(published.extraction.card_count, 2);
    let observations = state
        .semantic_memory()
        .list_observations(&context, extraction_id)
        .await
        .unwrap();
    assert_eq!(observations[0].kind, "unknown");
    let cards = service.list_cards(&context, &core, 20).await.unwrap();
    assert_eq!(cards.len(), 2);
    assert!(cards.iter().all(|card| card.items.len() >= 2));
    assert!(cards.iter().any(|card| {
        card.items
            .iter()
            .any(|item| item.observation_kind == "unknown")
    }));
    let file = state
        .files()
        .get_active(&context, &path)
        .await
        .unwrap()
        .unwrap();
    let source = state
        .semantic_memory()
        .get_source_by_file(&context, file.id)
        .await
        .unwrap()
        .unwrap();
    let organization_observations = state
        .semantic_organization()
        .current_observations(&context, &[source.source_id])
        .await
        .unwrap();
    assert!(
        organization_observations
            .iter()
            .any(|observation| observation.kind == "unknown")
    );
    assert!(
        !organization_observations
            .iter()
            .any(|observation| observation.kind == "state")
    );
}

#[tokio::test]
async fn a80_partial_batches_resume_after_local_failure_without_publishing_partial_cards() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/a80-resume.md").unwrap();
    let mut content = String::from("# A80 resume source\n");
    for index in 0..85 {
        content.push_str(&format!("Supported resume statement {index}.\n"));
    }
    core.create_bytes(
        &context,
        &path,
        content.as_bytes(),
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();

    let prepared = service
        .begin_generation_resumable(&context, &core, &path)
        .await
        .unwrap();
    let extraction_id = prepared.extraction.id;
    let batches = prepared.observation_batches().unwrap();
    service
        .bind_observation_batches(
            &context,
            &prepared,
            &batches,
            "fake-provider-binding",
            "semantic-cards-tracked-adr-m6-v14",
            "semantic-cards-m6-json-v10",
        )
        .await
        .unwrap();
    service
        .reserve_observation_batch_attempt(&context, &prepared, &batches[0])
        .await
        .unwrap();
    let first_index = batches[0].input["blocks"][0]["evidence_index"]
        .as_u64()
        .unwrap();
    let first_response = json!({
        "claims":[{"statement":"Supported resume statement 0.","evidence_indices":[first_index]}]
    });
    service
        .accept_observation_batch(
            &context,
            &core,
            &prepared,
            &batches[0],
            &first_response.to_string(),
        )
        .await
        .unwrap();

    service
        .reserve_observation_batch_attempt(&context, &prepared, &batches[1])
        .await
        .unwrap();
    service
        .fail_observation_batch_attempt(
            &context,
            &core,
            &prepared,
            &batches[1],
            "provider_schema_invalid",
        )
        .await
        .unwrap();
    assert!(
        state
            .semantic_memory()
            .list_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        state
            .semantic_memory()
            .get_extraction(&context, extraction_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "running"
    );

    // Reconstruct the service after a process restart. The first fragment is
    // loaded from State and skipped; the failed fragment gets the one source
    // regen reservation and no partial card has become visible.
    let resumed_service = SemanticMemoryService::new(state.clone());
    let resumed = resumed_service
        .begin_generation_resumable(&context, &core, &path)
        .await
        .unwrap();
    assert_eq!(resumed.extraction.id, extraction_id);
    let resumed_batches = resumed.observation_batches().unwrap();
    let rows = resumed_service
        .bind_observation_batches(
            &context,
            &resumed,
            &resumed_batches,
            "fake-provider-binding",
            "semantic-cards-tracked-adr-m6-v14",
            "semantic-cards-m6-json-v10",
        )
        .await
        .unwrap();
    assert_eq!(rows[0].state, "validated");
    assert_eq!(rows[1].state, "failed");
    assert_eq!(
        resumed_service
            .reserve_observation_batch_regen_attempt(
                &context,
                &core,
                &resumed,
                &resumed_batches[1],
            )
            .await
            .unwrap(),
        "dispatching"
    );
    let second_index = resumed_batches[1].input["blocks"][0]["evidence_index"]
        .as_u64()
        .unwrap();
    let second_response = json!({
        "claims":[{"statement":"Supported resume statement 80.","evidence_indices":[second_index]}]
    });
    resumed_service
        .accept_observation_batch(
            &context,
            &core,
            &resumed,
            &resumed_batches[1],
            &second_response.to_string(),
        )
        .await
        .unwrap();
    let published = resumed_service
        .finalize_observation_batches(&context, &core, &path, &resumed, 2)
        .await
        .unwrap();
    assert_eq!(published.extraction.state, "success_nonempty");
    assert_eq!(published.extraction.card_count, 2);
    assert_eq!(
        resumed_service
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .len(),
        2
    );

    // Simulate the later crash window: State and Vault publication committed,
    // but the Eval card/progress artifact had not yet been written.
    let recovered_service = SemanticMemoryService::new(state.clone());
    let completed = recovered_service
        .begin_generation_resumable(&context, &core, &path)
        .await
        .unwrap();
    assert_eq!(completed.extraction.id, extraction_id);
    let completed_batches = completed.observation_batches().unwrap();
    let completed_rows = recovered_service
        .bind_observation_batches(
            &context,
            &completed,
            &completed_batches,
            "fake-provider-binding",
            "semantic-cards-tracked-adr-m6-v14",
            "semantic-cards-m6-json-v10",
        )
        .await
        .unwrap();
    assert!(completed_rows.iter().all(|row| row.state == "validated"));
    let restored = recovered_service
        .resume_published_observation_batches(&context, &core, &path, &completed, 2)
        .await
        .unwrap();
    assert_eq!(restored.extraction.state, "success_nonempty");
    assert_eq!(restored.extraction.card_count, 2);
}

#[tokio::test]
async fn a80_success_empty_publication_resumes_without_a_new_provider_result() {
    let (_dir, state, context, core, service) = fixture().await;
    let path = VaultPath::parse("notes/a80-empty-resume.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# No durable memory\nOrdinary source text only.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let prepared = service
        .begin_generation_resumable(&context, &core, &path)
        .await
        .unwrap();
    let extraction_id = prepared.extraction.id;
    let batches = prepared.observation_batches().unwrap();
    service
        .bind_observation_batches(
            &context,
            &prepared,
            &batches,
            "fake-provider-binding",
            "semantic-cards-tracked-adr-m6-v14",
            "semantic-cards-m6-json-v10",
        )
        .await
        .unwrap();
    for batch in &batches {
        service
            .reserve_observation_batch_attempt(&context, &prepared, batch)
            .await
            .unwrap();
        service
            .accept_observation_batch(&context, &core, &prepared, batch, r#"{"claims":[]}"#)
            .await
            .unwrap();
    }
    let published = service
        .finalize_observation_batches(
            &context,
            &core,
            &path,
            &prepared,
            u32::try_from(batches.len()).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(published.extraction.state, "success_empty");

    let resumed_service = SemanticMemoryService::new(state);
    let resumed = resumed_service
        .begin_generation_resumable(&context, &core, &path)
        .await
        .unwrap();
    assert_eq!(resumed.extraction.id, extraction_id);
    let resumed_batches = resumed.observation_batches().unwrap();
    let rows = resumed_service
        .bind_observation_batches(
            &context,
            &resumed,
            &resumed_batches,
            "fake-provider-binding",
            "semantic-cards-tracked-adr-m6-v14",
            "semantic-cards-m6-json-v10",
        )
        .await
        .unwrap();
    assert!(rows.iter().all(|row| row.state == "validated"));
    let restored = resumed_service
        .resume_published_observation_batches(
            &context,
            &core,
            &path,
            &resumed,
            u32::try_from(resumed_batches.len()).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(restored.extraction.state, "success_empty");
    assert_eq!(restored.extraction.card_count, 0);
}

#[tokio::test]
async fn invalidation_fences_running_empty_and_publication_attempts() {
    let (_dir, state, context, core, service) = fixture().await;
    let source_path = VaultPath::parse("notes/fenced.md").unwrap();
    core.create_bytes(
        &context,
        &source_path,
        b"# Fenced\nKeep this decision.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &source_path)
        .await
        .unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":"Keep this decision.","scope":"project","assertion_status":"source_asserted","admission_reason":"decision","value_for_future_work":"decision","body_block_ids":[body.local_id.clone()]}],
        "cards":[{"title":"Fenced decision","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    let published = service
        .submit_proposal_json(&context, &core, &source_path, &proposal.to_string())
        .await
        .unwrap();
    let card = service
        .list_cards(&context, &core, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let source = state
        .semantic_memory()
        .get_source(&context, card.source_id)
        .await
        .unwrap()
        .unwrap();
    let revision = source.current_revision_id.unwrap();
    let running_id = mcp_vault_domain::ExtractionSetId::new();
    let (running, created) = state
        .semantic_memory()
        .start_extraction(
            &context,
            running_id,
            card.source_id,
            revision,
            &source.content_hash,
            "semantic-memory-m1-v1",
            "fence-test-idempotency",
            "fence-test-request",
        )
        .await
        .unwrap();
    assert!(created);
    assert_eq!(running.state, "running");
    assert!(
        service
            .invalidate_source(&context, source.file_id, "source_changed", None,)
            .await
            .unwrap()
    );
    assert!(
        state
            .semantic_memory()
            .finish_empty_or_nonpublishable(&context, running_id, "success_empty", None)
            .await
            .is_err()
    );
    assert!(
        service
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .is_empty()
    );

    let observations = state
        .semantic_memory()
        .list_observations(&context, published.extraction.id)
        .await
        .unwrap();
    let observation = observations.first().unwrap();
    let evidence = state
        .semantic_memory()
        .get_evidence(&context, observation.evidence_ref_id)
        .await
        .unwrap();
    // Evidence is already ineligible after the fence, so the old extraction
    // cannot be used to manufacture a fresh publication either.
    assert!(evidence.is_none());
    let result = state
        .semantic_memory()
        .prepare_publication(
            &context,
            running_id,
            &[SemanticObservationInput {
                id: observation.id,
                local_key: "old".to_owned(),
                kind: observation.kind.clone(),
                statement: observation.statement.clone(),
                scope: observation.scope.clone(),
                assertion_status: observation.assertion_status.clone(),
                source_time_scope: observation.source_time_scope.clone(),
                conditions: observation.conditions.clone(),
                exceptions: observation.exceptions.clone(),
                ordered_steps: observation.ordered_steps.clone(),
                result: observation.result.clone(),
                uncertainty: observation.uncertainty.clone(),
                admission_reason: observation.admission_reason.clone(),
                value_for_future_work: observation.value_for_future_work.clone(),
                evidence: SemanticEvidenceInput {
                    id: observation.evidence_ref_id,
                    body_spans: Vec::<SemanticSpanInput>::new(),
                    context_spans: Vec::<SemanticSpanInput>::new(),
                },
            }],
            &[SemanticCardRevisionInput {
                card_id: mcp_vault_domain::MemoryCardId::new(),
                card_revision_id: mcp_vault_domain::CardRevisionId::new(),
                expected_card_revision_id: None,
                revision_number: 1,
                topic_key: "fenced".to_owned(),
                title: "Fenced".to_owned(),
                kind: "decision".to_owned(),
                scope_ref: "project".to_owned(),
                assertion_status: "source_asserted".to_owned(),
                temporal_scope: serde_json::json!({}),
                composition_profile_id: "semantic-memory-m1-v1".to_owned(),
                canonical_path: VaultPath::parse("_semantic/fenced.md").unwrap(),
                canonical_markdown_hash: "hash".to_owned(),
                canonical_bytes: b"candidate".to_vec(),
                items: vec![SemanticCardItemInput {
                    id: mcp_vault_domain::CardItemId::new(),
                    observation_id: observation.id,
                    kind: "core_assertion".to_owned(),
                    ordinal: 0,
                    content: observation.statement.clone(),
                    evidence_ref_ids: vec![observation.evidence_ref_id],
                }],
            }],
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn prepared_snapshot_is_not_recovered_after_source_generation_changes() {
    let (_dir, state, context, core, service) = fixture().await;
    let source_path = VaultPath::parse("notes/recover.md").unwrap();
    core.create_bytes(
        &context,
        &source_path,
        b"# Recover\nKeep only the current source generation.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &source_path)
        .await
        .unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":"Keep only the current source generation.","scope":"project","assertion_status":"source_asserted","admission_reason":"generation fence","value_for_future_work":"avoid stale recovery","body_block_ids":[body.local_id.clone()]}],
        "cards":[{"title":"Generation fence","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    service
        .submit_proposal_json(&context, &core, &source_path, &proposal.to_string())
        .await
        .unwrap();
    let old_card = service
        .list_cards(&context, &core, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let source = state
        .semantic_memory()
        .get_source(&context, old_card.source_id)
        .await
        .unwrap()
        .unwrap();
    let revision = source.current_revision_id.unwrap();
    let extraction_id = mcp_vault_domain::ExtractionSetId::new();
    let (running, created) = state
        .semantic_memory()
        .start_extraction(
            &context,
            extraction_id,
            old_card.source_id,
            revision,
            &source.content_hash,
            "semantic-memory-m1-v1",
            "recover-stale-snapshot",
            "recover-stale-snapshot-request",
        )
        .await
        .unwrap();
    assert!(created);
    assert_eq!(running.state, "running");
    let observation_id = mcp_vault_domain::ObservationId::new();
    let evidence_id = mcp_vault_domain::EvidenceRefId::new();
    let snapshots = state
        .semantic_memory()
        .prepare_publication(
            &context,
            extraction_id,
            &[SemanticObservationInput {
                id: observation_id,
                local_key: "stale".to_owned(),
                kind: "decision".to_owned(),
                statement: "Keep only the current source generation.".to_owned(),
                scope: "project".to_owned(),
                assertion_status: "source_asserted".to_owned(),
                source_time_scope: serde_json::json!({}),
                conditions: Vec::new(),
                exceptions: Vec::new(),
                ordered_steps: Vec::new(),
                result: None,
                uncertainty: None,
                admission_reason: "generation fence".to_owned(),
                value_for_future_work: "avoid stale recovery".to_owned(),
                evidence: SemanticEvidenceInput {
                    id: evidence_id,
                    body_spans: vec![SemanticSpanInput {
                        start_byte: 0,
                        end_byte: 1,
                        content_hash: "span".to_owned(),
                    }],
                    context_spans: Vec::new(),
                },
            }],
            &[SemanticCardRevisionInput {
                card_id: mcp_vault_domain::MemoryCardId::new(),
                card_revision_id: mcp_vault_domain::CardRevisionId::new(),
                expected_card_revision_id: None,
                revision_number: 1,
                topic_key: "stale-recovery".to_owned(),
                title: "Stale recovery".to_owned(),
                kind: "decision".to_owned(),
                scope_ref: "project".to_owned(),
                assertion_status: "source_asserted".to_owned(),
                temporal_scope: serde_json::json!({}),
                composition_profile_id: "semantic-memory-m1-v1".to_owned(),
                canonical_path: VaultPath::parse("semantic-memory/cards/stale-recovery.md")
                    .unwrap(),
                canonical_markdown_hash: "stale-hash".to_owned(),
                canonical_bytes: b"stale".to_vec(),
                items: vec![SemanticCardItemInput {
                    id: mcp_vault_domain::CardItemId::new(),
                    observation_id,
                    kind: "core_assertion".to_owned(),
                    ordinal: 0,
                    content: "Keep only the current source generation.".to_owned(),
                    evidence_ref_ids: vec![evidence_id],
                }],
            }],
        )
        .await
        .unwrap();
    let snapshot = snapshots.first().unwrap();
    state
        .semantic_memory()
        .mark_snapshot_written(
            &context,
            snapshot.id,
            old_card.canonical_file_id,
            snapshot.proposed_file_revision,
        )
        .await
        .unwrap();

    let (newer, created) = state
        .semantic_memory()
        .start_extraction(
            &context,
            mcp_vault_domain::ExtractionSetId::new(),
            old_card.source_id,
            revision,
            &source.content_hash,
            "semantic-memory-m1-v1",
            "recover-commit-fence-newer",
            "recover-commit-fence-newer-request",
        )
        .await
        .unwrap();
    assert!(created);
    assert!(newer.extraction_commit_sequence > running.extraction_commit_sequence);
    assert!(
        service
            .recover_publication(&context, &core, extraction_id)
            .await
            .is_err()
    );
    assert_eq!(
        service.list_cards(&context, &core, 20).await.unwrap().len(),
        1
    );

    assert!(
        service
            .invalidate_source(&context, source.file_id, "permission_revoked", Some(1))
            .await
            .unwrap()
    );
    assert!(
        service
            .recover_publication(&context, &core, extraction_id)
            .await
            .is_err()
    );
    assert!(
        service
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        state
            .semantic_memory()
            .get_card(&context, snapshot.card_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn stale_source_preflight_skips_m1_core_recovery_and_keeps_journal_unreconciled() {
    let (_dir, state, context, core, service) = fixture().await;
    let source_path = VaultPath::parse("notes/m1-preflight.md").unwrap();
    core.create_bytes(
        &context,
        &source_path,
        b"# M1 preflight\nKeep the source fence.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &source_path)
        .await
        .unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":"Keep the source fence.","scope":"project","assertion_status":"source_asserted","admission_reason":"preflight","value_for_future_work":"preflight","body_block_ids":[body.local_id.clone()]}],
        "cards":[{"title":"M1 preflight","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    let failing_core = core.clone().with_failure_injector(Arc::new(FailOnce {
        phase: CommitPhase::RenameCommitted,
        fired: AtomicBool::new(false),
    }));
    assert!(
        service
            .submit_proposal_json(&context, &failing_core, &source_path, &proposal.to_string())
            .await
            .is_err()
    );
    assert_eq!(
        state.files().list_incomplete(&context).await.unwrap().len(),
        1
    );
    let file = core.read(&context, &source_path).await.unwrap().file;
    state
        .semantic_memory()
        .invalidate_source(&context, file.id, "source_changed", None)
        .await
        .unwrap();
    let pending = state
        .semantic_memory()
        .pending_extractions(&context)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    // The M2 recovery entry point must apply the same cross-type barrier and
    // cannot finalize this stale M1 RenameCommitted journal either.
    assert!(
        SemanticOrganizationService::new(state.clone())
            .recover_organization(&context, &core, OrganizationJobId::new())
            .await
            .is_err()
    );
    assert_eq!(
        state.files().list_incomplete(&context).await.unwrap().len(),
        1
    );
    assert!(
        service
            .recover_publication(&context, &core, pending[0])
            .await
            .is_err()
    );
    assert_eq!(
        state.files().list_incomplete(&context).await.unwrap().len(),
        1
    );
    assert!(
        service
            .list_cards(&context, &core, 20)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn prepared_core_journal_rejects_cancel_and_survives_reconnect_recovery() {
    let (dir, state, context, core, service, url) = file_backed_fixture().await;
    let source_path = VaultPath::parse("notes/cancel-journal.md").unwrap();
    core.create_bytes(
        &context,
        &source_path,
        b"# Cancel journal\nKeep the recovery barrier.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &source_path)
        .await
        .unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":"Keep the recovery barrier.","scope":"project","assertion_status":"source_asserted","admission_reason":"cancel journal","value_for_future_work":"preserve barrier","body_block_ids":[body.local_id.clone()]}],
        "cards":[{"title":"Cancel journal","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    let failing_core = core.clone().with_failure_injector(Arc::new(FailOnce {
        phase: CommitPhase::RenameCommitted,
        fired: AtomicBool::new(false),
    }));
    assert!(
        service
            .submit_proposal_json(&context, &failing_core, &source_path, &proposal.to_string())
            .await
            .is_err()
    );
    let pending = state
        .semantic_memory()
        .pending_extractions(&context)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert!(
        service
            .cancel_extraction(&context, pending[0])
            .await
            .is_err()
    );
    let file = core.read(&context, &source_path).await.unwrap().file;
    state
        .semantic_memory()
        .invalidate_source(&context, file.id, "source_changed", None)
        .await
        .unwrap();
    assert_eq!(
        state.files().list_incomplete(&context).await.unwrap().len(),
        1
    );
    state.close().await;

    let reopened = StateStore::connect_and_migrate(&url).await.unwrap();
    let reopened_core = VaultCore::new(
        reopened.clone(),
        dir.path().join("history"),
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
        Default::default(),
    );
    let reopened_service = SemanticMemoryService::new(reopened.clone());
    assert!(
        reopened_service
            .cancel_extraction(&context, pending[0])
            .await
            .is_err()
    );
    assert!(
        reopened_service
            .recover_pending_publications(&context, &reopened_core)
            .await
            .is_err()
    );
    assert_eq!(
        reopened
            .files()
            .list_incomplete(&context)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        reopened_service
            .list_cards(&context, &reopened_core, 20)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn same_file_same_hash_move_updates_navigation_without_new_generation() {
    let (_dir, state, context, core, service) = fixture().await;
    let old_path = VaultPath::parse("notes/move.md").unwrap();
    let new_path = VaultPath::parse("archive/move.md").unwrap();
    core.create_bytes(
        &context,
        &old_path,
        b"# Move\nThe identity survives a path move.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = service
        .prepare_source(&context, &core, &old_path)
        .await
        .unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":"The identity survives a path move.","scope":"project","assertion_status":"source_asserted","admission_reason":"stable identity","value_for_future_work":"keep navigation valid","body_block_ids":[body.local_id.clone()]}],
        "cards":[{"title":"Stable identity","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    service
        .submit_proposal_json(&context, &core, &old_path, &proposal.to_string())
        .await
        .unwrap();
    let before = service
        .list_cards(&context, &core, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let source_before = state
        .semantic_memory()
        .get_source(&context, before.source_id)
        .await
        .unwrap()
        .unwrap();
    let moved = core
        .move_entry(
            &context,
            &old_path,
            &new_path,
            core.read(&context, &old_path)
                .await
                .unwrap()
                .file
                .current_revision,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    assert_eq!(moved.file.id, source_before.file_id);
    let _ = service
        .prepare_source(&context, &core, &new_path)
        .await
        .unwrap();
    let source_after_move = state
        .semantic_memory()
        .get_source(&context, before.source_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        source_after_move.current_revision_id,
        source_before.current_revision_id
    );
    assert_eq!(
        source_after_move.source_generation,
        source_before.source_generation
    );
    assert_eq!(source_after_move.source_path, new_path);
    let after_move = service.list_cards(&context, &core, 20).await.unwrap();
    assert_eq!(after_move.len(), 1);
    assert_eq!(after_move[0].source_path, new_path);

    let changed = core
        .replace_bytes(
            &context,
            &new_path,
            moved.file.current_revision,
            b"# Move\nThe identity survives a path move and a new revision.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let input = service
        .prepare_source(&context, &core, &new_path)
        .await
        .unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":"The identity survives a path move and a new revision.","scope":"project","assertion_status":"source_asserted","admission_reason":"new revision","value_for_future_work":"re-extract current content","body_block_ids":[body.local_id.clone()]}],
        "cards":[{"title":"Stable identity","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    let submitted = service
        .submit_proposal_json(&context, &core, &new_path, &proposal.to_string())
        .await
        .unwrap();
    assert_eq!(submitted.extraction.state, "success_nonempty");
    let source_after_revision = state
        .semantic_memory()
        .get_source(&context, before.source_id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        source_after_revision.current_revision_id,
        source_before.current_revision_id
    );
    assert!(source_after_revision.source_generation > source_before.source_generation);
    assert_eq!(changed.file.id, source_before.file_id);
    assert_eq!(
        service.list_cards(&context, &core, 20).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn stale_source_rebind_fence_rejects_same_hash_move_without_regressing_navigation() {
    let (_dir, state, context, core, service) = fixture().await;
    let old_path = VaultPath::parse("notes/fenced-move.md").unwrap();
    let new_path = VaultPath::parse("archive/fenced-move.md").unwrap();
    core.create_bytes(
        &context,
        &old_path,
        b"# Fenced move\nThe current navigation must win.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let stale_file = core.read(&context, &old_path).await.unwrap().file;
    let (card, evidence_id) = publish_decision(
        &service,
        &context,
        &core,
        &old_path,
        "The current navigation must win.",
        "Fenced move",
    )
    .await;
    let source = state
        .semantic_memory()
        .get_source(&context, card.source_id)
        .await
        .unwrap()
        .unwrap();
    let moved = core
        .move_entry(
            &context,
            &old_path,
            &new_path,
            stale_file.current_revision,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    assert_eq!(moved.file.path, new_path);
    let stale = state
        .semantic_memory()
        .upsert_source_revision_fenced(
            &context,
            &stale_file,
            &source.content_hash,
            source.authorization_revision,
        )
        .await;
    assert!(matches!(stale, Err(StateError::Conflict)));
    let after_stale = state
        .semantic_memory()
        .get_source(&context, source.source_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_stale.source_path, new_path);
    assert_eq!(
        service.list_cards(&context, &core, 20).await.unwrap().len(),
        1
    );

    let mut prefixed_file = moved.file.clone();
    prefixed_file.content_hash = Some(format!("sha256:{}", source.content_hash));
    state
        .semantic_memory()
        .upsert_source_revision_fenced(
            &context,
            &prefixed_file,
            &source.content_hash,
            source.authorization_revision,
        )
        .await
        .unwrap();

    let current = service
        .reconcile_source_event(&context, &core, moved.file.id)
        .await
        .unwrap();
    assert_eq!(
        current.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::NavigationOnly
    );
    let after_current = state
        .semantic_memory()
        .get_source(&context, source.source_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_current.source_path, new_path);
    let card = service
        .list_cards(&context, &core, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(card.source_path, new_path);
    assert!(
        service
            .read_evidence(&context, &core, evidence_id)
            .await
            .unwrap()
            .is_some()
    );
}
