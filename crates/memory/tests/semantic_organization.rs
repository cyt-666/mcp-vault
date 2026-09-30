use mcp_vault_core::{CommitPhase, FailureInjector, VaultCore};
use mcp_vault_domain::{
    Actor, ComposedCardId, ComposedCardItemId, ComposedCardRevisionId, ExtractionSetId,
    OrganizationJobId, Revision, SemanticSourceId, SourcePlane, SupportGroupId, SupportMemberId,
    VaultContext, VaultId, VaultPath, VaultPathPolicy, VaultSlug, WritePrecondition,
};
use mcp_vault_memory::{SemanticMemoryService, semantic::organize::SemanticOrganizationService};
use mcp_vault_state::{
    ComposedCardItemInput, ComposedCardRevisionInput, RelationCandidateInput, StateStore,
    SupportGroupInput, SupportMemberInput, VaultStatus,
};
use mcp_vault_storage_fs::{DurabilityPolicy, StorageOptions};
use serde_json::json;
use sha2::{Digest, Sha256};
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
            Err("semantic recovery preflight fixture fault")
        } else {
            Ok(())
        }
    }
}

async fn fixture() -> (
    StateStore,
    VaultContext,
    VaultCore,
    SemanticMemoryService,
    SemanticOrganizationService,
) {
    let dir = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("organization").unwrap(),
        dir.path().join("vault"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&context, "organization", VaultStatus::Active)
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
    let memory = SemanticMemoryService::new(state.clone());
    let organization = SemanticOrganizationService::new(state.clone());
    (state, context, core, memory, organization)
}

async fn seed_source(
    state: &StateStore,
    context: &VaultContext,
    core: &VaultCore,
    memory: &SemanticMemoryService,
    path: &VaultPath,
    statement: &str,
) -> mcp_vault_state::SemanticSourceRecord {
    core.create_bytes(
        context,
        path,
        format!("# Decision\n{statement}\n").as_bytes(),
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = memory.prepare_source(context, core, path).await.unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({"outcome":"success_nonempty","observations":[{"kind":"decision","statement":statement,"scope":"project","assertion_status":"source_asserted","admission_reason":"source decision","value_for_future_work":"preserve the bounded decision","body_block_ids":[body.local_id.clone()]}],"cards":[{"title":"Recovery source","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]});
    memory
        .submit_proposal_json(context, core, path, &proposal.to_string())
        .await
        .unwrap();
    let file = core.read(context, path).await.unwrap().file;
    state
        .semantic_memory()
        .get_source_by_file(context, file.id)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn equivalent_sources_publish_one_composed_card_with_two_or_members() {
    let (state, context, core, memory, organization) = fixture().await;
    let first_path = VaultPath::parse("notes/first.md").unwrap();
    let second_path = VaultPath::parse("notes/second.md").unwrap();
    for path in [&first_path, &second_path] {
        core.create_bytes(
            &context,
            path,
            b"# Decision\nDeploy with rollback enabled.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        let input = memory.prepare_source(&context, &core, path).await.unwrap();
        let body = input.blocks.last().unwrap();
        let proposal = json!({"outcome":"success_nonempty","observations":[{"kind":"decision","statement":"Deploy with rollback enabled.","scope":"project","assertion_status":"source_asserted","admission_reason":"source decision","value_for_future_work":"preserve the bounded decision","body_block_ids":[body.local_id.clone()]}],"cards":[{"title":"Deployment decision","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]});
        memory
            .submit_proposal_json(&context, &core, path, &proposal.to_string())
            .await
            .unwrap();
    }
    let first = state
        .semantic_memory()
        .get_source_by_file(
            &context,
            core.read(&context, &first_path).await.unwrap().file.id,
        )
        .await
        .unwrap()
        .unwrap();
    let second = state
        .semantic_memory()
        .get_source_by_file(
            &context,
            core.read(&context, &second_path).await.unwrap().file.id,
        )
        .await
        .unwrap()
        .unwrap();
    let candidates = organization
        .discover_candidates(&context, &[first.source_id, second.source_id])
        .await
        .unwrap();
    assert_eq!(candidates.len(), 1);
    let forged = json!({"actions":[{"action":"create_composed_card","candidate_ids":[mcp_vault_domain::RelationCandidateId::new().to_string()],"card_ref":"forged","title":"Forged","unknown":true}]});
    assert!(
        organization
            .organize_json(
                &context,
                &core,
                &[first.source_id, second.source_id],
                &forged.to_string()
            )
            .await
            .is_err()
    );
    let forged_candidate = json!({"actions":[{"action":"create_composed_card","candidate_ids":[mcp_vault_domain::RelationCandidateId::new().to_string()],"card_ref":"forged","title":"Forged"}]});
    assert!(
        organization
            .organize_json(
                &context,
                &core,
                &[first.source_id, second.source_id],
                &forged_candidate.to_string()
            )
            .await
            .is_err()
    );
    let jobs = state
        .semantic_organization()
        .list_jobs(&context, 20)
        .await
        .unwrap();
    let failed_job = jobs
        .iter()
        .find(|job| job.lifecycle_state == "failed")
        .expect("forged candidate job is terminalized");
    assert_eq!(
        state
            .semantic_organization()
            .organization_candidate_states(&context, failed_job.id)
            .await
            .unwrap(),
        vec!["rejected".to_owned()]
    );
    assert!(
        state
            .semantic_organization()
            .organization_action_outcomes(&context, failed_job.id)
            .await
            .unwrap()
            .is_empty()
    );
    let proposal = json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidates[0].id.to_string()],"card_ref":"deployment","title":"Deployment decision","support_operator":"or"}]});
    organization
        .organize_json(
            &context,
            &core,
            &[first.source_id, second.source_id],
            &proposal.to_string(),
        )
        .await
        .unwrap();
    let cards = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
        .await
        .unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].items[0].support_groups[0].operator, "or");
    assert_eq!(cards[0].items[0].support_groups[0].members.len(), 2);
    let composed_evidence = cards[0].items[0].support_groups[0].members[0].evidence_ref_id;
    assert!(
        state
            .semantic_memory()
            .get_evidence(&context, composed_evidence)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn success_empty_withdraws_old_m2_support_preserving_independent_or_support() {
    let (state, context, core, memory, organization) = fixture().await;
    let paths = [
        VaultPath::parse("notes/empty-a.md").unwrap(),
        VaultPath::parse("notes/empty-b.md").unwrap(),
    ];
    let first = seed_source(
        &state,
        &context,
        &core,
        &memory,
        &paths[0],
        "Keep the empty-result fence.",
    )
    .await;
    let second = seed_source(
        &state,
        &context,
        &core,
        &memory,
        &paths[1],
        "Keep the empty-result fence.",
    )
    .await;
    let source_ids = [first.source_id, second.source_id];
    let candidate = organization
        .discover_candidates(&context, &source_ids)
        .await
        .unwrap()
        .pop()
        .unwrap();
    for operator in ["or", "and"] {
        let proposal = json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":format!("empty-{operator}"),"title":format!("Empty result {operator}"),"support_operator":operator}]});
        organization
            .organize_json(&context, &core, &source_ids, &proposal.to_string())
            .await
            .unwrap();
    }
    let before = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
        .await
        .unwrap();
    assert_eq!(before.len(), 2);
    let or_card = before
        .iter()
        .find(|card| card.items[0].support_groups[0].operator == "or")
        .unwrap();
    let first_evidence = or_card.items[0].support_groups[0].members[0].evidence_ref_id;
    let second_evidence = or_card.items[0].support_groups[0].members[1].evidence_ref_id;

    let empty = json!({"outcome":"success_empty","observations":[],"cards":[]});
    memory
        .submit_proposal_json(&context, &core, &paths[0], &empty.to_string())
        .await
        .unwrap();

    let after = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
        .await
        .unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].items[0].support_groups[0].operator, "or");
    assert_eq!(after[0].items[0].support_groups[0].members.len(), 1);
    assert!(
        state
            .semantic_memory()
            .get_evidence(&context, first_evidence)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        state
            .semantic_memory()
            .get_evidence(&context, second_evidence)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn composed_card_rule_does_not_hide_m1_or_another_composed_card() {
    let (state, context, core, memory, organization) = fixture().await;
    let paths = [
        VaultPath::parse("notes/scope-a.md").unwrap(),
        VaultPath::parse("notes/scope-b.md").unwrap(),
        VaultPath::parse("notes/scope-c.md").unwrap(),
    ];
    let sources = [
        seed_source(
            &state,
            &context,
            &core,
            &memory,
            &paths[0],
            "Scope isolated decision.",
        )
        .await,
        seed_source(
            &state,
            &context,
            &core,
            &memory,
            &paths[1],
            "Scope isolated decision.",
        )
        .await,
    ];
    let source_ids = sources
        .iter()
        .map(|source| source.source_id)
        .collect::<Vec<_>>();
    let candidates = organization
        .discover_candidates(&context, &source_ids)
        .await
        .unwrap();
    let candidate = candidates.into_iter().next().unwrap();
    for (title, card_ref, operator) in [
        ("First scoped card", "first-scoped", "or"),
        ("Second scoped card", "second-scoped", "and"),
    ] {
        let proposal = json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":card_ref,"title":title,"support_operator":operator}]});
        organization
            .organize_json(&context, &core, &source_ids, &proposal.to_string())
            .await
            .unwrap();
    }
    let cards = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
        .await
        .unwrap();
    assert_eq!(cards.len(), 2);
    let first_card = cards
        .iter()
        .find(|card| card.title == "First scoped card")
        .unwrap();
    let first_evidence = first_card.items[0].support_groups[0].members[0].evidence_ref_id;
    let first_target = state
        .semantic_rules()
        .list_targets(&context)
        .await
        .unwrap()
        .into_iter()
        .find(|target| {
            target.target_kind == "composed_card"
                && target
                    .fingerprint
                    .contains(&sources[0].source_id.to_string())
                && target
                    .fingerprint
                    .contains(&sources[1].source_id.to_string())
                && target
                    .fingerprint
                    .contains("\"core_assertion\",0,\"Scope isolated decision.\",\"or\",0")
        })
        .unwrap();
    state
        .semantic_rules()
        .apply_suppression(
            &context,
            "composed_card",
            "project",
            first_target.fingerprint_version,
            &first_target.fingerprint,
            &json!({"reason":"hide-one-composed-card"}),
            "suppress_read",
            None,
            None,
            None,
            None,
            Some(&first_card.id.to_string()),
            None,
            "test-actor",
            "hide-one-composed-card",
        )
        .await
        .unwrap();
    assert_eq!(
        state
            .semantic_memory()
            .list_cards(&context, 20)
            .await
            .unwrap()
            .len(),
        2
    );
    let remaining = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].title, "Second scoped card");
    assert!(
        state
            .semantic_memory()
            .get_evidence(&context, first_evidence)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn observation_rule_binding_must_belong_to_the_bound_composed_card() {
    let (state, context, core, memory, organization) = fixture().await;
    let first_path = VaultPath::parse("notes/binding-a.md").unwrap();
    let second_path = VaultPath::parse("notes/binding-b.md").unwrap();
    core.create_bytes(
        &context,
        &first_path,
        b"# Decisions\nA1 shared decision.\nA2 private decision.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = memory
        .prepare_source(&context, &core, &first_path)
        .await
        .unwrap();
    let first_body = input.blocks[input.blocks.len() - 2].local_id.clone();
    let second_body = input.blocks[input.blocks.len() - 1].local_id.clone();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[
            {"kind":"decision","statement":"A1 shared decision.","scope":"project","assertion_status":"source_asserted","admission_reason":"shared","value_for_future_work":"preserve the bounded decision","body_block_ids":[first_body]},
            {"kind":"decision","statement":"A2 private decision.","scope":"project","assertion_status":"source_asserted","admission_reason":"private","value_for_future_work":"keep private","body_block_ids":[second_body]}
        ],
        "cards":[
            {"title":"A1 card","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]},
            {"title":"A2 card","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[1]}
        ]
    });
    memory
        .submit_proposal_json(&context, &core, &first_path, &proposal.to_string())
        .await
        .unwrap();
    let second = seed_source(
        &state,
        &context,
        &core,
        &memory,
        &second_path,
        "A1 shared decision.",
    )
    .await;
    let first_file = core.read(&context, &first_path).await.unwrap().file;
    let first = state
        .semantic_memory()
        .get_source_by_file(&context, first_file.id)
        .await
        .unwrap()
        .unwrap();
    let source_cards = state
        .semantic_memory()
        .list_cards(&context, 20)
        .await
        .unwrap();
    let private_observation = source_cards
        .iter()
        .find(|card| card.title == "A2 card")
        .unwrap()
        .items[0]
        .observation_id;
    let private_card = source_cards
        .iter()
        .find(|card| card.title == "A2 card")
        .unwrap();
    let private_target = state
        .semantic_rules()
        .list_targets(&context)
        .await
        .unwrap()
        .into_iter()
        .find(|target| {
            target.target_kind == "memory_card"
                && target.fingerprint.contains("A2 private decision.")
        })
        .unwrap();
    state
        .semantic_rules()
        .apply_suppression(
            &context,
            "memory_card",
            "project",
            private_target.fingerprint_version,
            &private_target.fingerprint,
            &json!({"reason":"hide-private-card-only"}),
            "suppress_read",
            Some(&first.source_id.to_string()),
            Some(&first.current_revision_id.unwrap().to_string()),
            None,
            Some(&private_card.id.to_string()),
            None,
            None,
            "test-actor",
            "hide-private-card-only",
        )
        .await
        .unwrap();
    let candidate = organization
        .discover_candidates(&context, &[first.source_id, second.source_id])
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let proposal = json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":"binding","title":"Binding card","support_operator":"or"}]});
    organization
        .organize_json(
            &context,
            &core,
            &[first.source_id, second.source_id],
            &proposal.to_string(),
        )
        .await
        .unwrap();
    let composed = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
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
            target.target_kind == "composed_card"
                && target.fingerprint.contains(&first.source_id.to_string())
                && target.fingerprint.contains(&second.source_id.to_string())
        })
        .unwrap();
    assert!(
        state
            .semantic_rules()
            .apply_suppression(
                &context,
                "composed_card",
                "project",
                target.fingerprint_version,
                &target.fingerprint,
                &json!({"reason":"unrelated-observation"}),
                "suppress_regeneration",
                Some(&first.source_id.to_string()),
                Some(&first.current_revision_id.unwrap().to_string()),
                Some(&private_observation.to_string()),
                None,
                Some(&composed.id.to_string()),
                None,
                "test-actor",
                "unrelated-observation-binding",
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn added_supported_information_changes_target_but_reuses_card_identity() {
    let (state, context, core, memory, organization) = fixture().await;
    let paths = [
        VaultPath::parse("notes/item-a.md").unwrap(),
        VaultPath::parse("notes/item-b.md").unwrap(),
        VaultPath::parse("notes/item-c.md").unwrap(),
    ];
    let mut sources = Vec::new();
    for path in &paths {
        sources.push(
            seed_source(
                &state,
                &context,
                &core,
                &memory,
                path,
                "Keep the bounded decision.",
            )
            .await,
        );
    }
    let source_ids = sources
        .iter()
        .map(|source| source.source_id)
        .collect::<Vec<_>>();
    let candidates = organization
        .discover_candidates(&context, &source_ids)
        .await
        .unwrap();
    let candidate_between = |left: SemanticSourceId, right: SemanticSourceId| {
        candidates
            .iter()
            .find(|candidate| {
                (candidate.left.source_id == left && candidate.right.source_id == right)
                    || (candidate.left.source_id == right && candidate.right.source_id == left)
            })
            .unwrap()
    };
    let first_candidate = candidate_between(sources[0].source_id, sources[1].source_id);
    let added_candidate = candidate_between(sources[1].source_id, sources[2].source_id);
    let with_item = json!({
        "actions": [
            {"action":"create_composed_card","candidate_ids":[first_candidate.id.to_string()],"card_ref":"bounded","title":"Bounded decision","support_operator":"or"},
            {"action":"add_supported_information","candidate_ids":[added_candidate.id.to_string()],"card_ref":"bounded","content":"Only after approval","item_kind":"condition"}
        ]
    });
    organization
        .organize_json(&context, &core, &source_ids, &with_item.to_string())
        .await
        .unwrap();
    let first_card = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();

    let without_item = json!({
        "actions": [{"action":"create_composed_card","candidate_ids":[first_candidate.id.to_string()],"card_ref":"bounded-renamed","title":"Bounded decision","support_operator":"or"}]
    });
    organization
        .organize_json(&context, &core, &source_ids, &without_item.to_string())
        .await
        .unwrap();
    let cards = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
        .await
        .unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].id, first_card.id);
    assert_eq!(cards[0].revision_number, first_card.revision_number + 1);
    assert_eq!(
        state
            .semantic_rules()
            .list_targets(&context)
            .await
            .unwrap()
            .into_iter()
            .filter(|target| target.target_kind == "composed_card")
            .count(),
        2,
        "adding/removing supported information must split semantic targets"
    );
}

#[tokio::test]
async fn source_invalidation_preserves_or_but_not_and_support() {
    let (state, context, core, memory, organization) = fixture().await;
    let paths = [
        VaultPath::parse("notes/a.md").unwrap(),
        VaultPath::parse("notes/b.md").unwrap(),
    ];
    for path in &paths {
        core.create_bytes(
            &context,
            path,
            b"# Decision\nDeploy with rollback enabled.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        let input = memory.prepare_source(&context, &core, path).await.unwrap();
        let body = input.blocks.last().unwrap();
        let proposal = json!({"outcome":"success_nonempty","observations":[{"kind":"decision","statement":"Deploy with rollback enabled.","scope":"project","assertion_status":"source_asserted","admission_reason":"source decision","value_for_future_work":"preserve the bounded decision","body_block_ids":[body.local_id.clone()]}],"cards":[{"title":"Deployment decision","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]});
        memory
            .submit_proposal_json(&context, &core, path, &proposal.to_string())
            .await
            .unwrap();
    }
    let first_file = core.read(&context, &paths[0]).await.unwrap().file;
    let second_file = core.read(&context, &paths[1]).await.unwrap().file;
    let first = state
        .semantic_memory()
        .get_source_by_file(&context, first_file.id)
        .await
        .unwrap()
        .unwrap();
    let second = state
        .semantic_memory()
        .get_source_by_file(&context, second_file.id)
        .await
        .unwrap()
        .unwrap();
    let ids = [first.source_id, second.source_id];
    let candidate = organization
        .discover_candidates(&context, &ids)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let proposal = json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":"deployment","title":"Deployment decision","support_operator":"or"}]});
    organization
        .organize_json(&context, &core, &ids, &proposal.to_string())
        .await
        .unwrap();
    let first_composed = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let renamed_proposal = json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":"deployment-renamed","title":"Deployment decision renamed","support_operator":"or"}]});
    organization
        .organize_json(&context, &core, &ids, &renamed_proposal.to_string())
        .await
        .unwrap();
    let composed_after_rebuild = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
        .await
        .unwrap();
    assert_eq!(
        composed_after_rebuild.len(),
        1,
        "same core structure reuses one composed card despite card_ref/title changes"
    );
    assert_eq!(composed_after_rebuild[0].id, first_composed.id);
    assert_eq!(
        composed_after_rebuild[0].revision_number,
        first_composed.revision_number + 1
    );
    assert!(
        state
            .semantic_organization()
            .list_composed_cards(&context, 20)
            .await
            .unwrap()
            .iter()
            .any(|card| card.title == "Deployment decision renamed")
    );
    let and_proposal = json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":"deployment-and","title":"Deployment decision renamed","support_operator":"and"}]});
    organization
        .organize_json(&context, &core, &ids, &and_proposal.to_string())
        .await
        .unwrap();
    assert_eq!(
        state
            .semantic_organization()
            .list_composed_cards(&context, 20)
            .await
            .unwrap()
            .len(),
        2,
        "changing support structure creates a separate composed card"
    );
    assert_eq!(
        state
            .semantic_rules()
            .list_targets(&context)
            .await
            .unwrap()
            .into_iter()
            .filter(|target| target.target_kind == "composed_card")
            .count(),
        2,
        "title changes reuse the semantic card, while support-operator changes split the target"
    );
    let source_cards = state
        .semantic_memory()
        .list_cards(&context, 20)
        .await
        .unwrap();
    let first_card = source_cards
        .iter()
        .find(|card| card.source_id == first.source_id)
        .unwrap();
    let evidence_id = first_card.items[0].evidence_ref_ids[0];
    let rules = state.semantic_rules();
    let source_target = rules
        .list_targets(&context)
        .await
        .unwrap()
        .into_iter()
        .find(|target| {
            target.target_kind == "memory_card"
                && target.fingerprint.contains(&first.source_id.to_string())
                && target.fingerprint.contains("Deploy with rollback enabled.")
        })
        .expect("stable M1 semantic target");
    let first_source_id = first.source_id.to_string();
    let first_revision_id = first.current_revision_id.unwrap().to_string();
    let first_card_id = first_card.id.to_string();
    let _source_suppression = rules
        .apply_suppression(
            &context,
            "memory_card",
            "project",
            source_target.fingerprint_version,
            &source_target.fingerprint,
            &json!({"reason":"hide-source-card"}),
            "suppress_read",
            Some(&first_source_id),
            Some(&first_revision_id),
            None,
            Some(&first_card_id),
            None,
            None,
            "test-actor",
            "hide-source-card",
        )
        .await
        .unwrap();
    assert!(
        state
            .semantic_memory()
            .get_card(&context, first_card.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        state
            .semantic_memory()
            .get_evidence(&context, evidence_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        state
            .semantic_memory()
            .list_cards(&context, 20)
            .await
            .unwrap()
            .len(),
        1
    );
    let composed_after_source_suppression = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
        .await
        .unwrap();
    assert_eq!(composed_after_source_suppression.len(), 1);
    assert_eq!(
        composed_after_source_suppression[0].items[0].support_groups[0]
            .members
            .len(),
        1,
        "an OR card keeps unrelated valid evidence after one source is suppressed"
    );
    let surviving_evidence =
        composed_after_source_suppression[0].items[0].support_groups[0].members[0].evidence_ref_id;
    assert!(
        state
            .semantic_memory()
            .get_evidence(&context, surviving_evidence)
            .await
            .unwrap()
            .is_some()
    );
    let composed_target = rules
        .list_targets(&context)
        .await
        .unwrap()
        .into_iter()
        .find(|target| {
            target.target_kind == "composed_card" && target.fingerprint.contains("\"or\"")
        })
        .unwrap();
    let correction = rules
        .apply_correction(
            &context,
            "composed_card",
            "project",
            composed_target.fingerprint_version,
            &composed_target.fingerprint,
            &json!({"replace":"bounded decision","remove":"Deploy with rollback enabled."}),
            None,
            None,
            None,
            None,
            None,
            Some(rules.current_rules_revision(&context).await.unwrap()),
            "test-actor",
            "block-composed-correction",
        )
        .await
        .unwrap();
    let blocked_by_correction = json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":"blocked-correction","title":"Deployment decision corrected","support_operator":"or"}]});
    assert!(
        organization
            .organize_json(&context, &core, &ids, &blocked_by_correction.to_string())
            .await
            .is_err()
    );
    rules
        .revoke_correction(
            &context,
            &correction.id,
            correction.revision,
            "unblock-composed-correction",
            "test-actor",
        )
        .await
        .unwrap();
    let _regeneration = rules
        .apply_suppression(
            &context,
            "composed_card",
            "project",
            composed_target.fingerprint_version,
            &composed_target.fingerprint,
            &json!({"reason":"do-not-regenerate"}),
            "suppress_regeneration",
            None,
            None,
            None,
            None,
            None,
            Some(rules.current_rules_revision(&context).await.unwrap()),
            "test-actor",
            "block-composed-regeneration",
        )
        .await
        .unwrap();
    let blocked_by_regeneration = json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":"blocked-regeneration","title":"Deployment decision regenerated","support_operator":"or"}]});
    assert!(
        organization
            .organize_json(&context, &core, &ids, &blocked_by_regeneration.to_string())
            .await
            .is_err()
    );
    let composed_suppression = rules
        .apply_suppression(
            &context,
            "composed_card",
            "project",
            composed_target.fingerprint_version,
            &composed_target.fingerprint,
            &json!({"reason":"hide-composed"}),
            "suppress_read",
            None,
            None,
            None,
            None,
            None,
            None,
            "test-actor",
            "hide-composed-card",
        )
        .await
        .unwrap();
    rules
        .revoke_suppression(
            &context,
            &composed_suppression.id,
            composed_suppression.revision,
            "restore-composed-card",
            "test-actor",
        )
        .await
        .unwrap();
    let first_revision = core
        .read(&context, &paths[0])
        .await
        .unwrap()
        .file
        .current_revision;
    core.delete(
        &context,
        &paths[0],
        first_revision,
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        state
            .semantic_organization()
            .list_composed_cards(&context, 20)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        state
            .semantic_organization()
            .list_composed_cards(&context, 20)
            .await
            .unwrap()[0]
            .title,
        "Deployment decision renamed"
    );
    let second_revision = core
        .read(&context, &paths[1])
        .await
        .unwrap()
        .file
        .current_revision;
    core.delete(
        &context,
        &paths[1],
        second_revision,
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    assert!(
        state
            .semantic_organization()
            .list_composed_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn local_rebind_withdraws_old_m2_and_keeps_independent_or_support() {
    let (state, context, core, memory, organization) = fixture().await;
    let first_path = VaultPath::parse("notes/rebind-m2-a.md").unwrap();
    let second_path = VaultPath::parse("notes/rebind-m2-b.md").unwrap();
    let first = seed_source(
        &state,
        &context,
        &core,
        &memory,
        &first_path,
        "Deploy with rollback enabled.",
    )
    .await;
    let second = seed_source(
        &state,
        &context,
        &core,
        &memory,
        &second_path,
        "Deploy with rollback enabled.",
    )
    .await;
    let ids = [first.source_id, second.source_id];
    let candidate = organization
        .discover_candidates(&context, &ids)
        .await
        .unwrap()
        .pop()
        .unwrap();
    for (card_ref, operator) in [("rebind-m2-or", "or"), ("rebind-m2-and", "and")] {
        let proposal = json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":card_ref,"title":card_ref,"support_operator":operator}]});
        organization
            .organize_json(&context, &core, &ids, &proposal.to_string())
            .await
            .unwrap();
    }
    assert_eq!(
        state
            .semantic_organization()
            .list_composed_cards(&context, 20)
            .await
            .unwrap()
            .len(),
        2
    );

    let first_file = core.read(&context, &first_path).await.unwrap().file;
    let changed = core
        .replace_bytes(
            &context,
            &first_path,
            first_file.current_revision,
            b"# Decision\nAn unrelated paragraph.\nDeploy with rollback enabled.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let report = memory
        .reconcile_source_event(&context, &core, changed.file.id)
        .await
        .unwrap();
    assert_eq!(
        report.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::Rebound
    );
    let readable = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
        .await
        .unwrap();
    assert_eq!(readable.len(), 1);
    assert_eq!(readable[0].items[0].support_groups[0].operator, "or");
    assert_eq!(
        readable[0].items[0].support_groups[0].members.len(),
        1,
        "the current second-source OR support remains readable"
    );
}

#[tokio::test]
async fn three_source_suppression_is_precise_before_discovery_and_prepare() {
    let (state, context, core, memory, organization) = fixture().await;
    let first = seed_source(
        &state,
        &context,
        &core,
        &memory,
        &VaultPath::parse("notes/precise-a.md").unwrap(),
        "Keep the bounded decision.",
    )
    .await;
    let second = seed_source(
        &state,
        &context,
        &core,
        &memory,
        &VaultPath::parse("notes/precise-b.md").unwrap(),
        "Keep the bounded decision.",
    )
    .await;
    let third = seed_source(
        &state,
        &context,
        &core,
        &memory,
        &VaultPath::parse("notes/precise-c.md").unwrap(),
        "Keep the bounded decision.",
    )
    .await;
    let source_card = state
        .semantic_memory()
        .list_cards(&context, 20)
        .await
        .unwrap()
        .into_iter()
        .find(|card| card.source_id == first.source_id)
        .unwrap();
    let source_target = state
        .semantic_rules()
        .list_targets(&context)
        .await
        .unwrap()
        .into_iter()
        .find(|target| {
            target.target_kind == "memory_card"
                && target.fingerprint.contains(&first.source_id.to_string())
        })
        .unwrap();
    state
        .semantic_rules()
        .apply_suppression(
            &context,
            "memory_card",
            "project",
            source_target.fingerprint_version,
            &source_target.fingerprint,
            &json!({"reason":"hide-one-source"}),
            "suppress_read",
            Some(&first.source_id.to_string()),
            Some(&first.current_revision_id.unwrap().to_string()),
            None,
            Some(&source_card.id.to_string()),
            None,
            None,
            "test-actor",
            "precise-source-suppression",
        )
        .await
        .unwrap();

    let first_and_second = [first.source_id, second.source_id];
    assert!(
        organization
            .discover_candidates(&context, &first_and_second)
            .await
            .unwrap()
            .is_empty(),
        "suppressed observations must disappear before a new proposal is prepared"
    );
    let second_and_third = [second.source_id, third.source_id];
    let candidate = organization
        .discover_candidates(&context, &second_and_third)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("unrelated sources remain discoverable");
    let composed_rule = state
        .semantic_rules()
        .apply_suppression(
            &context,
            "composed_card",
            "project",
            1,
            "m2-source-fence",
            &json!({"reason":"do-not-regenerate-from-one-source"}),
            "suppress_regeneration",
            Some(&first.source_id.to_string()),
            Some(&first.current_revision_id.unwrap().to_string()),
            None,
            None,
            None,
            None,
            "test-actor",
            "precise-m2-source-fence",
        )
        .await
        .unwrap();
    let proposal = json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":"precise-three-source","title":"Bounded decision","support_operator":"or"}]});
    organization
        .organize_json(
            &context,
            &core,
            &[first.source_id, second.source_id, third.source_id],
            &proposal.to_string(),
        )
        .await
        .unwrap();
    let composed = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
        .await
        .unwrap();
    assert_eq!(composed.len(), 1);
    assert_eq!(composed[0].items[0].support_groups[0].members.len(), 2);
    assert_eq!(composed_rule.action, "suppress_regeneration");
}

#[tokio::test]
async fn running_recovery_terminalizes_without_snapshot_and_batch_continues() {
    let (state, context, core, memory, organization) = fixture().await;
    let path = VaultPath::parse("notes/recovery.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Decision\nKeep the recovery fence.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let input = memory.prepare_source(&context, &core, &path).await.unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({"outcome":"success_nonempty","observations":[{"kind":"decision","statement":"Keep the recovery fence.","scope":"project","assertion_status":"source_asserted","admission_reason":"source decision","value_for_future_work":"preserve the bounded decision","body_block_ids":[body.local_id.clone()]}],"cards":[{"title":"Recovery fence","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]});
    memory
        .submit_proposal_json(&context, &core, &path, &proposal.to_string())
        .await
        .unwrap();
    let file = core.read(&context, &path).await.unwrap().file;
    let source = state
        .semantic_memory()
        .get_source_by_file(&context, file.id)
        .await
        .unwrap()
        .unwrap();
    let fence = state
        .semantic_organization()
        .current_source_fences(&context, &[source.source_id])
        .await
        .unwrap();
    let repository = state.semantic_organization();
    let first = OrganizationJobId::new();
    let second = OrganizationJobId::new();
    for (job, key) in [(first, "recovery-first"), (second, "recovery-second")] {
        repository
            .start_job(
                &context,
                job,
                &format!("input-{key}"),
                0,
                "semantic-memory-m2-v1",
                key,
                &format!("request-{key}"),
                &fence,
            )
            .await
            .unwrap();
    }
    assert!(
        organization
            .recover_pending_organizations(&context, &core)
            .await
            .is_err()
    );
    for job in [first, second] {
        let record = repository.get_job(&context, job).await.unwrap().unwrap();
        assert_eq!(record.lifecycle_state, "blocked");
        assert_eq!(
            record.safe_error_code.as_deref(),
            Some("organization_recovery_without_snapshot")
        );
    }
}

#[tokio::test]
async fn written_recovery_conflict_is_blocked_and_batch_continues() {
    let (state, context, core, memory, organization) = fixture().await;
    let first = seed_source(
        &state,
        &context,
        &core,
        &memory,
        &VaultPath::parse("notes/written-a.md").unwrap(),
        "Keep the recovery fence.",
    )
    .await;
    let second = seed_source(
        &state,
        &context,
        &core,
        &memory,
        &VaultPath::parse("notes/written-b.md").unwrap(),
        "Keep the recovery fence.",
    )
    .await;
    let repository = state.semantic_organization();
    let source_ids = [first.source_id, second.source_id];
    let observations = organization
        .discover_candidates(&context, &source_ids)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let fences = repository
        .current_source_fences(&context, &source_ids)
        .await
        .unwrap();
    let job_id = OrganizationJobId::new();
    repository
        .start_job(
            &context,
            job_id,
            "written-input",
            0,
            "semantic-memory-m2-v1",
            "written-job",
            "written-request",
            &fences,
        )
        .await
        .unwrap();
    repository
        .record_candidate(
            &context,
            job_id,
            &RelationCandidateInput {
                id: mcp_vault_domain::RelationCandidateId::new(),
                left_observation_id: observations.left.id,
                left_source_id: observations.left.source_id,
                left_source_revision_id: observations.left.source_revision_id,
                right_observation_id: observations.right.id,
                right_source_id: observations.right.source_id,
                right_source_revision_id: observations.right.source_revision_id,
                candidate_input_hash: "written-candidate".to_owned(),
                similarity_hint: None,
                profile_id: "semantic-memory-m2-v1".to_owned(),
            },
        )
        .await
        .unwrap();
    let bytes = b"# Recovery witness\n".to_vec();
    let card = ComposedCardRevisionInput {
        card_id: ComposedCardId::new(),
        card_revision_id: ComposedCardRevisionId::new(),
        expected_card_revision_id: None,
        revision_number: 1,
        identity_key: "recovery-witness".to_owned(),
        topic_key: "recovery-witness".to_owned(),
        title: "Recovery witness".to_owned(),
        kind: "decision".to_owned(),
        scope_ref: "project".to_owned(),
        assertion_status: "source_asserted".to_owned(),
        temporal_scope: json!({}),
        composition_profile_id: "semantic-memory-m2-v1".to_owned(),
        canonical_path: core
            .managed_root()
            .join(&VaultPath::parse("semantic-memory/composed/recovery-witness.md").unwrap())
            .unwrap(),
        canonical_markdown_hash: Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        canonical_bytes: bytes,
        items: vec![ComposedCardItemInput {
            id: ComposedCardItemId::new(),
            kind: "core_assertion".to_owned(),
            ordinal: 0,
            content: "Keep the recovery fence.".to_owned(),
            support_groups: vec![SupportGroupInput {
                id: SupportGroupId::new(),
                operator: "or".to_owned(),
                ordinal: 0,
                members: vec![SupportMemberInput {
                    id: SupportMemberId::new(),
                    source_id: first.source_id,
                    source_revision_id: first.current_revision_id.unwrap(),
                    observation_id: observations.left.id,
                    evidence_ref_id: observations.left.evidence_ref_id,
                    member_role: "complete".to_owned(),
                }],
            }],
        }],
        dependencies: fences
            .iter()
            .map(|fence| (fence.source_id, fence.source_revision_id))
            .collect(),
    };
    let snapshot = repository
        .prepare_organization(&context, job_id, &[], &[card])
        .await
        .unwrap()
        .pop()
        .unwrap();
    let written = core
        .create_managed_bytes(
            &context,
            &snapshot.target_path,
            &snapshot.canonical_bytes,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    repository
        .mark_snapshot_written(
            &context,
            snapshot.id,
            written.file.id,
            written.file.current_revision,
            &snapshot.proposed_file_hash,
        )
        .await
        .unwrap();
    let fresh_fence = repository
        .current_source_fences(&context, &[second.source_id])
        .await
        .unwrap();
    let pending_job = OrganizationJobId::new();
    repository
        .start_job(
            &context,
            pending_job,
            "pending-input",
            0,
            "semantic-memory-m2-v1",
            "pending-job",
            "pending-request",
            &fresh_fence,
        )
        .await
        .unwrap();
    let reached = std::sync::Arc::new(tokio::sync::Notify::new());
    let proceed = std::sync::Arc::new(tokio::sync::Notify::new());
    let recovery = {
        let organization = organization.clone();
        let context = context.clone();
        let core = core.clone();
        let reached = reached.clone();
        let proceed = proceed.clone();
        tokio::spawn(async move {
            organization
                .recover_organization_with_apply_gate(&context, &core, job_id, reached, proceed)
                .await
        })
    };
    reached.notified().await;
    state
        .semantic_memory()
        .invalidate_source(&context, first.file_id, "source_changed", None)
        .await
        .unwrap();
    proceed.notify_one();
    assert!(recovery.await.unwrap().is_err());
    let written_job = repository.get_job(&context, job_id).await.unwrap().unwrap();
    assert_eq!(written_job.lifecycle_state, "blocked");
    assert_eq!(
        written_job.safe_error_code.as_deref(),
        Some("semantic_recovery_apply_failed")
    );
    assert!(!repository.pending_jobs(&context).await.unwrap().is_empty());
    assert!(
        organization
            .recover_pending_organizations(&context, &core)
            .await
            .is_err()
    );
    let pending_job = repository
        .get_job(&context, pending_job)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending_job.lifecycle_state, "blocked");
    assert!(repository.pending_jobs(&context).await.unwrap().is_empty());
    assert!(
        repository
            .list_composed_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn stale_source_preflight_skips_core_recovery_and_keeps_journal_unreconciled() {
    let (state, context, core, memory, organization) = fixture().await;
    let source = seed_source(
        &state,
        &context,
        &core,
        &memory,
        &VaultPath::parse("notes/source-preflight.md").unwrap(),
        "Keep the source preflight fence.",
    )
    .await;
    let repository = state.semantic_organization();
    let fences = repository
        .current_source_fences(&context, &[source.source_id])
        .await
        .unwrap();
    let job_id = OrganizationJobId::new();
    repository
        .start_job(
            &context,
            job_id,
            "source-preflight-input",
            0,
            "semantic-memory-m2-v1",
            "source-preflight-job",
            "source-preflight-request",
            &fences,
        )
        .await
        .unwrap();
    let source_card = state
        .semantic_memory()
        .list_cards(&context, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let bytes = b"# Source preflight\n".to_vec();
    let card = ComposedCardRevisionInput {
        card_id: ComposedCardId::new(),
        card_revision_id: ComposedCardRevisionId::new(),
        expected_card_revision_id: None,
        revision_number: 1,
        identity_key: "source-preflight-card".to_owned(),
        topic_key: "source-preflight-card".to_owned(),
        title: "Source preflight".to_owned(),
        kind: "decision".to_owned(),
        scope_ref: "project".to_owned(),
        assertion_status: "source_asserted".to_owned(),
        temporal_scope: json!({}),
        composition_profile_id: "semantic-memory-m2-v1".to_owned(),
        canonical_path: core
            .managed_root()
            .join(&VaultPath::parse("semantic-memory/composed/source-preflight.md").unwrap())
            .unwrap(),
        canonical_markdown_hash: Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        canonical_bytes: bytes,
        items: vec![ComposedCardItemInput {
            id: ComposedCardItemId::new(),
            kind: "core_assertion".to_owned(),
            ordinal: 0,
            content: "Keep the source preflight fence.".to_owned(),
            support_groups: vec![SupportGroupInput {
                id: SupportGroupId::new(),
                operator: "or".to_owned(),
                ordinal: 0,
                members: vec![SupportMemberInput {
                    id: SupportMemberId::new(),
                    source_id: source.source_id,
                    source_revision_id: source.current_revision_id.unwrap(),
                    observation_id: source_card.items[0].observation_id,
                    evidence_ref_id: source_card.items[0].evidence_ref_ids[0],
                    member_role: "complete".to_owned(),
                }],
            }],
        }],
        dependencies: vec![(source.source_id, source.current_revision_id.unwrap())],
    };
    let snapshot = repository
        .prepare_organization(&context, job_id, &[], &[card])
        .await
        .unwrap()
        .pop()
        .unwrap();
    let failing_core = core.clone().with_failure_injector(Arc::new(FailOnce {
        phase: CommitPhase::RenameCommitted,
        fired: AtomicBool::new(false),
    }));
    assert!(
        failing_core
            .create_managed_bytes(
                &context,
                &snapshot.target_path,
                &snapshot.canonical_bytes,
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await
            .is_err()
    );
    assert_eq!(
        state.files().list_incomplete(&context).await.unwrap().len(),
        1
    );
    state
        .semantic_memory()
        .invalidate_source(&context, source.file_id, "source_changed", None)
        .await
        .unwrap();
    assert!(
        organization
            .recover_organization(&context, &core, job_id)
            .await
            .is_err()
    );
    // The M1 recovery entry point must apply the same cross-type barrier and
    // cannot finalize this stale M2 RenameCommitted journal either.
    assert!(
        memory
            .recover_publication(&context, &core, ExtractionSetId::new())
            .await
            .is_err()
    );
    assert_eq!(
        state.files().list_incomplete(&context).await.unwrap().len(),
        1
    );
    assert!(
        repository
            .list_composed_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn normal_organization_core_failure_remains_a_barrier_on_recovery() {
    let (state, context, core, memory, organization) = fixture().await;
    let first = seed_source(
        &state,
        &context,
        &core,
        &memory,
        &VaultPath::parse("notes/normal-failure-a.md").unwrap(),
        "Keep the normal organization fence.",
    )
    .await;
    let second = seed_source(
        &state,
        &context,
        &core,
        &memory,
        &VaultPath::parse("notes/normal-failure-b.md").unwrap(),
        "Keep the normal organization fence.",
    )
    .await;
    let candidate = organization
        .discover_candidates(&context, &[first.source_id, second.source_id])
        .await
        .unwrap()
        .pop()
        .unwrap();
    let proposal = json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":"normal-failure","title":"Normal failure","support_operator":"or"}]});
    let failing_core = core.clone().with_failure_injector(Arc::new(FailOnce {
        phase: CommitPhase::RenameCommitted,
        fired: AtomicBool::new(false),
    }));
    assert!(
        organization
            .organize_json(
                &context,
                &failing_core,
                &[first.source_id, second.source_id],
                &proposal.to_string(),
            )
            .await
            .is_err()
    );
    assert_eq!(
        state.files().list_incomplete(&context).await.unwrap().len(),
        1
    );
    let job = state
        .semantic_organization()
        .list_jobs(&context, 20)
        .await
        .unwrap()
        .into_iter()
        .find(|job| job.lifecycle_state == "blocked")
        .expect("normal Core failure job is durable");
    assert_eq!(job.lifecycle_state, "blocked");

    state
        .semantic_memory()
        .invalidate_source(&context, first.file_id, "source_changed", None)
        .await
        .unwrap();
    assert!(
        organization
            .recover_pending_organizations(&context, &core)
            .await
            .is_err()
    );
    assert_eq!(
        state.files().list_incomplete(&context).await.unwrap().len(),
        1,
        "stale semantic journal must not be finalized by generic Core recovery"
    );
    assert!(
        state
            .semantic_organization()
            .list_composed_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn stale_rules_preflight_skips_core_recovery_and_keeps_journal_unreconciled() {
    let (state, context, core, memory, organization) = fixture().await;
    let first = seed_source(
        &state,
        &context,
        &core,
        &memory,
        &VaultPath::parse("notes/preflight-a.md").unwrap(),
        "Keep the preflight fence.",
    )
    .await;
    let second = seed_source(
        &state,
        &context,
        &core,
        &memory,
        &VaultPath::parse("notes/preflight-b.md").unwrap(),
        "Keep the preflight fence.",
    )
    .await;
    let repository = state.semantic_organization();
    let source_ids = [first.source_id, second.source_id];
    let candidate = organization
        .discover_candidates(&context, &source_ids)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let fences = repository
        .current_source_fences(&context, &source_ids)
        .await
        .unwrap();
    let job_id = OrganizationJobId::new();
    repository
        .start_job(
            &context,
            job_id,
            "preflight-input",
            0,
            "semantic-memory-m2-v1",
            "preflight-job",
            "preflight-request",
            &fences,
        )
        .await
        .unwrap();
    repository
        .record_candidate(
            &context,
            job_id,
            &RelationCandidateInput {
                id: mcp_vault_domain::RelationCandidateId::new(),
                left_observation_id: candidate.left.id,
                left_source_id: candidate.left.source_id,
                left_source_revision_id: candidate.left.source_revision_id,
                right_observation_id: candidate.right.id,
                right_source_id: candidate.right.source_id,
                right_source_revision_id: candidate.right.source_revision_id,
                candidate_input_hash: "preflight-candidate".to_owned(),
                similarity_hint: None,
                profile_id: "semantic-memory-m2-v1".to_owned(),
            },
        )
        .await
        .unwrap();
    let bytes = b"# Preflight witness\n".to_vec();
    let card = ComposedCardRevisionInput {
        card_id: ComposedCardId::new(),
        card_revision_id: ComposedCardRevisionId::new(),
        expected_card_revision_id: None,
        revision_number: 1,
        identity_key: "preflight-witness".to_owned(),
        topic_key: "preflight-witness".to_owned(),
        title: "Preflight witness".to_owned(),
        kind: "decision".to_owned(),
        scope_ref: "project".to_owned(),
        assertion_status: "source_asserted".to_owned(),
        temporal_scope: json!({}),
        composition_profile_id: "semantic-memory-m2-v1".to_owned(),
        canonical_path: core
            .managed_root()
            .join(&VaultPath::parse("semantic-memory/composed/preflight-witness.md").unwrap())
            .unwrap(),
        canonical_markdown_hash: Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        canonical_bytes: bytes,
        items: vec![ComposedCardItemInput {
            id: ComposedCardItemId::new(),
            kind: "core_assertion".to_owned(),
            ordinal: 0,
            content: "Keep the preflight fence.".to_owned(),
            support_groups: vec![SupportGroupInput {
                id: SupportGroupId::new(),
                operator: "or".to_owned(),
                ordinal: 0,
                members: vec![SupportMemberInput {
                    id: SupportMemberId::new(),
                    source_id: first.source_id,
                    source_revision_id: first.current_revision_id.unwrap(),
                    observation_id: candidate.left.id,
                    evidence_ref_id: candidate.left.evidence_ref_id,
                    member_role: "complete".to_owned(),
                }],
            }],
        }],
        dependencies: vec![(first.source_id, first.current_revision_id.unwrap())],
    };
    let snapshot = repository
        .prepare_organization(&context, job_id, &[], &[card])
        .await
        .unwrap()
        .pop()
        .unwrap();
    let failing_core = core.clone().with_failure_injector(Arc::new(FailOnce {
        phase: CommitPhase::RenameCommitted,
        fired: AtomicBool::new(false),
    }));
    assert!(
        failing_core
            .create_managed_bytes(
                &context,
                &snapshot.target_path,
                &snapshot.canonical_bytes,
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await
            .is_err()
    );
    assert_eq!(
        state.files().list_incomplete(&context).await.unwrap().len(),
        1
    );

    state
        .semantic_rules()
        .apply_suppression(
            &context,
            "composed_card",
            "project",
            1,
            "preflight-rule",
            &json!({"reason":"stale-rules"}),
            "suppress_regeneration",
            None,
            None,
            None,
            None,
            None,
            None,
            "test-actor",
            "stale-rules-preflight",
        )
        .await
        .unwrap();
    assert!(
        organization
            .recover_organization(&context, &core, job_id)
            .await
            .is_err()
    );
    assert_eq!(
        state.files().list_incomplete(&context).await.unwrap().len(),
        1
    );
    assert!(
        repository
            .list_composed_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
    let job = repository.get_job(&context, job_id).await.unwrap().unwrap();
    assert_eq!(job.lifecycle_state, "blocked");
    assert_eq!(
        job.safe_error_code.as_deref(),
        Some("semantic_rules_changed")
    );
}

#[tokio::test]
async fn organization_rules_survive_file_backed_reconnect_and_keep_republish_unreadable() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("organization-rules.sqlite3");
    let url = format!("sqlite://{}", database.display());
    let state = StateStore::connect_and_migrate(&url).await.unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("organization-rules-reconnect").unwrap(),
        directory.path().join("vault"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&context, "organization rules", VaultStatus::Active)
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
    let history = directory.path().join("history");
    let core = VaultCore::new(
        state.clone(),
        history.clone(),
        VaultPathPolicy::default(),
        StorageOptions::default(),
        Default::default(),
    );
    let memory = SemanticMemoryService::new(state.clone());
    let organization = SemanticOrganizationService::new(state.clone());
    let mut sources = Vec::new();
    for (ordinal, path) in [
        (1_u8, VaultPath::parse("notes/organization-a.md").unwrap()),
        (2_u8, VaultPath::parse("notes/organization-b.md").unwrap()),
    ] {
        core.create_bytes(
            &context,
            &path,
            b"# Decision\nKeep the organization boundary.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        let input = memory.prepare_source(&context, &core, &path).await.unwrap();
        let body = input.blocks.last().unwrap();
        let proposal = json!({"outcome":"success_nonempty","observations":[{"kind":"decision","statement":"Keep the organization boundary.","scope":"project","assertion_status":"source_asserted","admission_reason":"organization reconnect","value_for_future_work":"preserve the boundary","body_block_ids":[body.local_id.clone()]}],"cards":[{"title":format!("Organization source {ordinal}"),"kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]});
        memory
            .submit_proposal_json(&context, &core, &path, &proposal.to_string())
            .await
            .unwrap();
        let file = core.read(&context, &path).await.unwrap().file;
        sources.push(
            state
                .semantic_memory()
                .get_source_by_file(&context, file.id)
                .await
                .unwrap()
                .unwrap(),
        );
    }
    let candidate = organization
        .discover_candidates(&context, &[sources[0].source_id, sources[1].source_id])
        .await
        .unwrap()
        .pop()
        .unwrap();
    let proposal = json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":"reconnect-boundary","title":"Organization boundary","support_operator":"or"}]});
    organization
        .organize_json(
            &context,
            &core,
            &[sources[0].source_id, sources[1].source_id],
            &proposal.to_string(),
        )
        .await
        .unwrap();
    let composed = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
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
        .find(|target| target.target_kind == "composed_card")
        .unwrap();
    state
        .semantic_rules()
        .apply_suppression(
            &context,
            "composed_card",
            "project",
            target.fingerprint_version,
            &target.fingerprint,
            &json!({"reason":"organization-reconnect"}),
            "forget_current",
            None,
            None,
            None,
            None,
            Some(&composed.id.to_string()),
            None,
            "test-actor",
            "organization-reconnect",
        )
        .await
        .unwrap();
    state
        .semantic_rules()
        .apply_suppression(
            &context,
            "composed_card",
            "project",
            target.fingerprint_version,
            &target.fingerprint,
            &json!({"reason":"organization-reconnect-regeneration"}),
            "suppress_regeneration",
            Some(&sources[0].source_id.to_string()),
            Some(&sources[0].current_revision_id.unwrap().to_string()),
            None,
            None,
            None,
            None,
            "test-actor",
            "organization-reconnect-regeneration",
        )
        .await
        .unwrap();
    state.close().await;

    let reopened = StateStore::connect_and_migrate(&url).await.unwrap();
    let reopened_core = VaultCore::new(
        reopened.clone(),
        history,
        VaultPathPolicy::default(),
        StorageOptions::default(),
        Default::default(),
    );
    let reopened_organization = SemanticOrganizationService::new(reopened.clone());
    assert!(
        reopened
            .semantic_organization()
            .list_composed_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
    let mut ids = Vec::new();
    for path in ["notes/organization-a.md", "notes/organization-b.md"] {
        let file = reopened_core
            .read(&context, &VaultPath::parse(path).unwrap())
            .await
            .unwrap()
            .file;
        let source_id = reopened
            .semantic_memory()
            .get_source_by_file(&context, file.id)
            .await
            .unwrap()
            .unwrap()
            .source_id;
        ids.push(source_id);
    }
    let candidate = reopened_organization
        .discover_candidates(&context, &ids)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let old_proposal = json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":"reconnect-boundary","title":"Organization boundary","support_operator":"or"}]});
    let replay = reopened_organization
        .organize_json(&context, &reopened_core, &ids, &old_proposal.to_string())
        .await;
    if replay.is_ok() {
        assert!(
            reopened
                .semantic_organization()
                .list_composed_cards(&context, 20)
                .await
                .unwrap()
                .is_empty()
        );
    }
    reopened_organization
        .recover_pending_organizations(&context, &reopened_core)
        .await
        .unwrap();
    assert!(
        reopened
            .semantic_organization()
            .list_composed_cards(&context, 20)
            .await
            .unwrap()
            .is_empty()
    );
}
