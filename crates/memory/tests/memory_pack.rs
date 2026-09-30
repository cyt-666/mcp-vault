use mcp_vault_core::VaultCore;
use mcp_vault_domain::{
    Actor, Revision, SourcePlane, VaultContext, VaultId, VaultPath, VaultPathPolicy, VaultSlug,
    WritePrecondition,
};
use mcp_vault_indexer::IndexService;
use mcp_vault_memory::{
    MemoryPackRequest, MemoryPackService, SemanticMemoryService,
    semantic::organize::SemanticOrganizationService,
};
use mcp_vault_state::{StateStore, VaultStatus};
use mcp_vault_storage_fs::{DurabilityPolicy, StorageOptions};
use serde_json::json;

async fn fixture(slug: &str) -> (tempfile::TempDir, StateStore, VaultContext, VaultCore) {
    let dir = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new(slug).unwrap(),
        dir.path().join("vault"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&context, slug, VaultStatus::Active)
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
    (dir, state, context, core)
}

async fn publish(
    state: &StateStore,
    context: &VaultContext,
    core: &VaultCore,
    path: &VaultPath,
    statement: &str,
) -> mcp_vault_state::SemanticCardRecord {
    publish_scoped(state, context, core, path, statement, "project").await
}

async fn publish_scoped(
    state: &StateStore,
    context: &VaultContext,
    core: &VaultCore,
    path: &VaultPath,
    statement: &str,
    scope: &str,
) -> mcp_vault_state::SemanticCardRecord {
    let memory = SemanticMemoryService::new(state.clone());
    let input = memory.prepare_source(context, core, path).await.unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":statement,"scope":scope,"assertion_status":"source_asserted","conditions":["approval is required"],"exceptions":["emergency changes"],"ordered_steps":["review","apply"],"admission_reason":"pack test","value_for_future_work":"preserve qualifiers","body_block_ids":[body.local_id.clone()]}],
        "cards":[{"title":"Pack decision","kind":"decision","scope":scope,"assertion_status":"source_asserted","observation_indices":[0]}]
    });
    memory
        .submit_proposal_json(context, core, path, &proposal.to_string())
        .await
        .unwrap();
    state
        .semantic_memory()
        .list_cards(context, 20)
        .await
        .unwrap()
        .pop()
        .unwrap()
}

#[tokio::test]
async fn memory_pack_is_vault_scoped_and_keeps_semantic_qualifiers() {
    let (_first_dir, first_state, first, first_core) = fixture("pack-first").await;
    let (_second_dir, second_state, second, second_core) = fixture("pack-second").await;
    let first_path = VaultPath::parse("notes/decision.md").unwrap();
    let second_path = VaultPath::parse("notes/decision.md").unwrap();
    for (state, context, core, path) in [
        (&first_state, &first, &first_core, &first_path),
        (&second_state, &second, &second_core, &second_path),
    ] {
        core.create_bytes(
            context,
            path,
            b"# Private heading\nThe decision is kept private.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        publish(state, context, core, path, "The decision is kept private.").await;
    }
    let pack = MemoryPackService::new(first_state.clone())
        .build(
            &first,
            &first_core,
            &MemoryPackRequest {
                task: "decision".to_owned(),
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(pack.current_context.len(), 1);
    assert_eq!(
        pack.current_context[0].core_assertions,
        ["The decision is kept private."]
    );
    assert_eq!(pack.current_context[0].conditions, ["approval is required"]);
    assert_eq!(pack.current_context[0].exceptions, ["emergency changes"]);
    assert_eq!(pack.current_context[0].ordered_steps, ["review", "apply"]);
    assert!(
        pack.current_context[0].source_references[0]
            .source_path
            .as_deref()
            == Some("notes/decision.md")
    );
    assert!(pack.current_context[0].optional_details[0] != "# Private heading");
}

#[tokio::test]
async fn memory_pack_filters_scope_path_and_unknown_version_without_approximation() {
    let (_dir, state, context, core) = fixture("pack-filters").await;
    let path = VaultPath::parse("notes/project.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Project\nKeep the project decision.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    publish(&state, &context, &core, &path, "Keep the project decision.").await;
    let service = MemoryPackService::new(state);
    let mismatch = service
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "decision".to_owned(),
                exact_source_path: Some("notes/other.md".to_owned()),
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(mismatch.current_context.is_empty());
    assert!(
        mismatch
            .evidence_gaps
            .iter()
            .any(|gap| gap.code == "no_answer")
    );
    let unknown_version = service
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "decision".to_owned(),
                version: Some("v9".to_owned()),
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(unknown_version.current_context.is_empty());
    assert!(
        unknown_version
            .evidence_gaps
            .iter()
            .any(|gap| gap.code == "version_or_as_of_unknown")
    );
    let subject_mismatch = service
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "decision".to_owned(),
                subject_terms: vec!["unrelated-subject".to_owned()],
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(
        subject_mismatch
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic == "subject_mismatch")
    );
    let prefix_mismatch = service
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "decision".to_owned(),
                path_prefix: Some("notes/proj".to_owned()),
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(
        prefix_mismatch
            .evidence_gaps
            .iter()
            .any(|gap| gap.code == "no_answer")
    );
}

#[tokio::test]
async fn memory_pack_suppression_and_history_are_fail_closed() {
    let (_dir, state, context, core) = fixture("pack-rules").await;
    let path = VaultPath::parse("notes/rules.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Rules\nKeep the rule decision.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let card = publish(&state, &context, &core, &path, "Keep the rule decision.").await;
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
            &json!({"reason":"pack suppression"}),
            "suppress_read",
            None,
            None,
            None,
            Some(&card.id.to_string()),
            None,
            None,
            "test",
            "pack-suppression",
        )
        .await
        .unwrap();
    let service = MemoryPackService::new(state);
    let suppressed = service
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "rule".to_owned(),
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(suppressed.current_context.is_empty());
    let history = service
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "rule".to_owned(),
                mode: mcp_vault_memory::pack::MemoryPackMode::History,
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(
        history
            .evidence_gaps
            .iter()
            .any(|gap| gap.code == "history_mode_unsupported")
    );
}

#[tokio::test]
async fn memory_pack_rejects_disabled_policy_and_corrupt_managed_card_file() {
    let (_dir, state, context, core) = fixture("pack-canonical").await;
    let path = VaultPath::parse("notes/canonical.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Canonical\nKeep the canonical decision.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let card = publish(
        &state,
        &context,
        &core,
        &path,
        "Keep the canonical decision.",
    )
    .await;
    let service = MemoryPackService::new(state.clone());
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
            &json!({"enabled":false,"request_timeout_seconds":300}),
            WritePrecondition::ExactRevision(policy.revision),
            None,
        )
        .await
        .unwrap();
    let disabled = service
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "canonical".to_owned(),
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(disabled.current_context.is_empty());
    assert!(
        disabled
            .evidence_gaps
            .iter()
            .any(|gap| gap.code == "policy_unavailable")
    );

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
    core.replace_managed_bytes(
        &context,
        &card.canonical_path,
        card.canonical_file_revision,
        b"tampered managed card",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let corrupt = service
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "canonical".to_owned(),
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(corrupt.current_context.is_empty());
}

#[tokio::test]
async fn memory_pack_uses_current_m2_or_support_after_and_support_stales() {
    let (_dir, state, context, core) = fixture("pack-m2").await;
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
        publish(
            &state,
            &context,
            &core,
            path,
            "Deploy with rollback enabled.",
        )
        .await;
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
    let organization = SemanticOrganizationService::new(state.clone());
    let candidate = organization
        .discover_candidates(&context, &[first.source_id, second.source_id])
        .await
        .unwrap()
        .pop()
        .unwrap();
    for (card_ref, operator) in [("pack-or", "or"), ("pack-and", "and")] {
        organization
            .organize_json(
                &context,
                &core,
                &[first.source_id, second.source_id],
                &json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":card_ref,"title":"Deploy with rollback enabled","support_operator":operator}]}).to_string(),
            )
            .await
            .unwrap();
    }
    let current = core.read(&context, &first_path).await.unwrap().file;
    let changed = core
        .replace_bytes(
            &context,
            &first_path,
            current.current_revision,
            b"# Decision\nAn unrelated update.\nDeploy with rollback enabled.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    SemanticMemoryService::new(state.clone())
        .reconcile_source_event(&context, &core, changed.file.id)
        .await
        .unwrap();
    let pack = MemoryPackService::new(state.clone())
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "rollback".to_owned(),
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(pack.current_context.len(), 1);
    assert!(
        pack.current_context[0]
            .source_references
            .iter()
            .any(|source| source.source_id == second.source_id.to_string())
    );
    let m2_only = MemoryPackService::new(state)
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "rollback".to_owned(),
                include_m1: false,
                exact_source_path: Some("notes/second.md".to_owned()),
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(m2_only.current_context.len(), 1);
    assert!(
        m2_only.current_context[0]
            .source_references
            .iter()
            .all(|source| source.source_path.as_deref() == Some("notes/second.md"))
    );
}

#[tokio::test]
async fn memory_pack_outputs_an_accepted_conflict_check_without_deduping_it() {
    let (_dir, state, context, core) = fixture("pack-conflict").await;
    let left_path = VaultPath::parse("notes/left.md").unwrap();
    let right_path = VaultPath::parse("notes/right.md").unwrap();
    for (path, statement) in [(&left_path, "Use rollout."), (&right_path, "Use rollout.")] {
        core.create_bytes(
            &context,
            path,
            format!("# Rollout\n{statement}\n").as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        publish(&state, &context, &core, path, statement).await;
    }
    let left = state
        .semantic_memory()
        .get_source_by_file(
            &context,
            core.read(&context, &left_path).await.unwrap().file.id,
        )
        .await
        .unwrap()
        .unwrap();
    let right = state
        .semantic_memory()
        .get_source_by_file(
            &context,
            core.read(&context, &right_path).await.unwrap().file.id,
        )
        .await
        .unwrap()
        .unwrap();
    let organization = SemanticOrganizationService::new(state.clone());
    let candidate = organization
        .discover_candidates(&context, &[left.source_id, right.source_id])
        .await
        .unwrap()
        .pop()
        .unwrap();
    let left_observation_id = candidate.left.id;
    let right_observation_id = candidate.right.id;
    let candidate_id = candidate.id;
    let hook_state = state.clone();
    let hook_context = context.clone();
    let hook_core = core.clone();
    let pack = MemoryPackService::new(state)
        .build_with_test_hook(
            &context,
            &core,
            &MemoryPackRequest {
                task: "rollout".to_owned(),
                ..MemoryPackRequest::default()
            },
            move || async move {
                SemanticOrganizationService::new(hook_state)
                    .organize_json(
                        &hook_context,
                        &hook_core,
                        &[left.source_id, right.source_id],
                        &json!({"actions":[{"action":"record_conflict","candidate_ids":[candidate_id.to_string()],"reason":"rollout choices conflict"}]}).to_string(),
                    )
                    .await
            },
        )
        .await
        .unwrap();
    assert!(
        pack.conflicts_or_checks
            .iter()
            .any(|check| check.relation_kind == "conflicts"
                && check.left_observation_id != check.right_observation_id
                && check.source_ids.len() == 2)
    );
    assert_eq!(pack.current_context.len(), 2);
    assert!(pack.conflicts_or_checks.iter().any(|check| {
        check.left_observation_id == left_observation_id.to_string()
            && check.right_observation_id == right_observation_id.to_string()
            && check.source_ids == vec![left.source_id.to_string(), right.source_id.to_string()]
    }));
}

#[tokio::test]
async fn memory_pack_outputs_an_accepted_supersedes_check_without_deduping_it() {
    let (_dir, state, context, core) = fixture("pack-supersedes").await;
    let left_path = VaultPath::parse("notes/old.md").unwrap();
    let right_path = VaultPath::parse("notes/new.md").unwrap();
    for (path, statement) in [
        (&left_path, "Use rollout A."),
        (&right_path, "Use rollout B."),
    ] {
        core.create_bytes(
            &context,
            path,
            format!("# Rollout\n{statement}\n").as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        publish(&state, &context, &core, path, statement).await;
    }
    let left = state
        .semantic_memory()
        .get_source_by_file(
            &context,
            core.read(&context, &left_path).await.unwrap().file.id,
        )
        .await
        .unwrap()
        .unwrap();
    let right = state
        .semantic_memory()
        .get_source_by_file(
            &context,
            core.read(&context, &right_path).await.unwrap().file.id,
        )
        .await
        .unwrap()
        .unwrap();
    let organization = SemanticOrganizationService::new(state.clone());
    let candidate = organization
        .discover_candidates(&context, &[left.source_id, right.source_id])
        .await
        .unwrap()
        .pop()
        .unwrap();
    let left_observation_id = candidate.left.id;
    let right_observation_id = candidate.right.id;
    organization
        .organize_json(
            &context,
            &core,
            &[left.source_id, right.source_id],
            &json!({"actions":[{"action":"supersede_with_evidence","candidate_ids":[candidate.id.to_string()],"card_ref":"current","title":"Current rollout","reason":"The source explicitly switches to B"}]}).to_string(),
        )
        .await
        .unwrap();
    let pack = MemoryPackService::new(state)
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "rollout".to_owned(),
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(pack.conflicts_or_checks.iter().any(|check| {
        check.relation_kind == "supersedes"
            && check.left_observation_id == left_observation_id.to_string()
            && check.right_observation_id == right_observation_id.to_string()
            && check.source_ids == vec![left.source_id.to_string(), right.source_id.to_string()]
    }));
    assert!(pack.current_context.len() + pack.relevant_experiences.len() >= 2);
}

#[tokio::test]
async fn memory_pack_outputs_an_accepted_different_scope_check_without_deduping_it() {
    let (_dir, state, context, core) = fixture("pack-different-scope").await;
    let left_path = VaultPath::parse("notes/project.md").unwrap();
    let right_path = VaultPath::parse("notes/team.md").unwrap();
    for path in [&left_path, &right_path] {
        core.create_bytes(
            &context,
            path,
            b"# Scope\nUse the scoped rollout.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    }
    publish_scoped(
        &state,
        &context,
        &core,
        &left_path,
        "Use the scoped rollout.",
        "project",
    )
    .await;
    publish_scoped(
        &state,
        &context,
        &core,
        &right_path,
        "Use the scoped rollout.",
        "task",
    )
    .await;
    let left = state
        .semantic_memory()
        .get_source_by_file(
            &context,
            core.read(&context, &left_path).await.unwrap().file.id,
        )
        .await
        .unwrap()
        .unwrap();
    let right = state
        .semantic_memory()
        .get_source_by_file(
            &context,
            core.read(&context, &right_path).await.unwrap().file.id,
        )
        .await
        .unwrap()
        .unwrap();
    let organization = SemanticOrganizationService::new(state.clone());
    let candidate = organization
        .discover_candidates(&context, &[left.source_id, right.source_id])
        .await
        .unwrap()
        .pop()
        .unwrap();
    let left_observation_id = candidate.left.id;
    let right_observation_id = candidate.right.id;
    organization
        .organize_json(
            &context,
            &core,
            &[left.source_id, right.source_id],
            &json!({"actions":[{"action":"keep_separate_scope","candidate_ids":[candidate.id.to_string()]}]}).to_string(),
        )
        .await
        .unwrap();
    let pack = MemoryPackService::new(state)
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "scoped rollout".to_owned(),
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(pack.conflicts_or_checks.iter().any(|check| {
        check.relation_kind == "different_scope"
            && check.left_observation_id == left_observation_id.to_string()
            && check.right_observation_id == right_observation_id.to_string()
            && check.source_ids == vec![left.source_id.to_string(), right.source_id.to_string()]
    }));
    assert!(pack.current_context.len() + pack.relevant_experiences.len() >= 2);
}

#[tokio::test]
async fn memory_pack_engineering_abcs_reuse_one_fixture_and_budget() {
    let (_dir, state, context, core) = fixture("pack-abc").await;
    let path = VaultPath::parse("notes/abc.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Ordinary note\nKeep the bounded decision.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    publish(&state, &context, &core, &path, "Keep the bounded decision.").await;
    let request = MemoryPackRequest {
        task: "bounded decision".to_owned(),
        max_entries: 4,
        max_bytes: 8_000,
        max_tokens: 2_000,
        ..MemoryPackRequest::default()
    };
    let baseline =
        mcp_vault_state::memory_search_terms(["# Ordinary note\nKeep the bounded decision."], 32);
    assert!(baseline.contains("bounded"));
    let m1 = MemoryPackService::new(state.clone())
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                include_m2: false,
                ..request.clone()
            },
        )
        .await
        .unwrap();
    let combined = MemoryPackService::new(state)
        .build(&context, &core, &request)
        .await
        .unwrap();
    assert_eq!(m1.current_context.len(), combined.current_context.len());
    assert!(combined.current_context[0].core_assertions[0].contains("bounded decision"));
}

#[tokio::test]
async fn memory_pack_abc_runner_uses_real_indexer_baseline() {
    let (_dir, state, context, core) = fixture("pack-abc-runner").await;
    let path = VaultPath::parse("notes/abc-runner.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Ordinary cue\nKeep the runner decision.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    publish(&state, &context, &core, &path, "Keep the runner decision.").await;
    let index = IndexService::new(state.clone());
    index.rebuild_vault(&core, &context).await.unwrap();
    let missing_manifest = MemoryPackService::new(state.clone())
        .compare_abc(
            &context,
            &core,
            &index,
            &MemoryPackRequest {
                task: "runner decision".to_owned(),
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(missing_manifest.iter().all(|record| {
        record
            .gap_codes
            .iter()
            .any(|code| code == "comparison_source_manifest_required")
    }));
    let records = MemoryPackService::new(state.clone())
        .compare_abc(
            &context,
            &core,
            &index,
            &MemoryPackRequest {
                task: "runner decision".to_owned(),
                comparison_source_paths: vec!["notes/abc-runner.md".to_owned()],
                max_entries: 4,
                max_bytes: 8_000,
                max_tokens: 2_000,
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        records
            .iter()
            .map(|record| record.mode.as_str())
            .collect::<Vec<_>>(),
        ["A", "B", "C"]
    );
    assert!(records[0].candidate_count > 0);
    assert_eq!(records[0].input_hash, records[1].input_hash);
    assert_eq!(records[1].input_hash, records[2].input_hash);
    assert!(records.iter().all(|record| record.manifest_count > 0));
    assert!(
        records
            .iter()
            .all(|record| record.manifest_hash == records[0].manifest_hash)
    );
    assert!(records.iter().all(|record| record.pending_real_evaluation));

    let unknown = MemoryPackService::new(state.clone())
        .compare_abc(
            &context,
            &core,
            &index,
            &MemoryPackRequest {
                task: "task-with-no-matching-source".to_owned(),
                comparison_source_paths: vec!["notes/abc-runner.md".to_owned()],
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(unknown[0].returned_count, 0);
    assert!(
        unknown[0]
            .gap_codes
            .iter()
            .any(|code| code == "ordinary_no_answer")
    );
    assert!(unknown[1..].iter().all(|record| record.returned_count == 0));

    let overflow = MemoryPackService::new(state.clone())
        .compare_abc(
            &context,
            &core,
            &index,
            &MemoryPackRequest {
                task: "runner decision".to_owned(),
                comparison_source_paths: vec!["notes/abc-runner.md".to_owned()],
                max_bytes: 64,
                max_tokens: 16,
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(overflow.iter().all(|record| {
        record.returned_count == 0
            && record
                .gap_codes
                .iter()
                .any(|code| code == "comparison_budget_exceeded")
    }));

    let degraded = MemoryPackService::new(state.clone())
        .compare_abc(
            &context,
            &core,
            &index,
            &MemoryPackRequest {
                task: "runner decision".to_owned(),
                comparison_source_paths: vec!["notes/abc-runner.md".to_owned()],
                scope_ref: Some("project".to_owned()),
                subject_terms: vec!["runner".to_owned()],
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(
        degraded[0]
            .gap_codes
            .iter()
            .any(|code| code == "ordinary_scope_unavailable")
    );
    assert!(
        degraded[0]
            .gap_codes
            .iter()
            .any(|code| code == "ordinary_subject_unavailable")
    );
}

#[tokio::test]
async fn memory_pack_abc_rejects_file_revision_mutation_against_stale_index() {
    let (_dir, state, context, core) = fixture("pack-abc-manifest-fence").await;
    let path = VaultPath::parse("notes/manifest.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Manifest\nKeep the old manifest decision.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    publish(
        &state,
        &context,
        &core,
        &path,
        "Keep the old manifest decision.",
    )
    .await;
    let index = IndexService::new(state.clone());
    index.rebuild_vault(&core, &context).await.unwrap();
    let file = core.read(&context, &path).await.unwrap().file;
    core.replace_bytes(
        &context,
        &path,
        file.current_revision,
        b"# Manifest\nThe manifest decision changed before comparison.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let records = MemoryPackService::new(state)
        .compare_abc(
            &context,
            &core,
            &index,
            &MemoryPackRequest {
                task: "manifest decision".to_owned(),
                comparison_source_paths: vec!["notes/manifest.md".to_owned()],
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(
        records.iter().all(|record| {
            record.candidate_count == 0
                && record.returned_count == 0
                && record.degraded
                && record
                    .gap_codes
                    .iter()
                    .any(|code| code == "source_manifest_changed_before_response")
        }),
        "unexpected revision-fence records: {records:#?}"
    );
}

#[tokio::test]
async fn memory_pack_abc_rebinds_same_revision_memory_to_the_moved_path() {
    let (_dir, state, context, core) = fixture("pack-abc-path-fence").await;
    let old_path = VaultPath::parse("notes/old-path.md").unwrap();
    let new_path = VaultPath::parse("notes/new-path.md").unwrap();
    core.create_bytes(
        &context,
        &old_path,
        b"# Move\nKeep the moved decision.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    publish(
        &state,
        &context,
        &core,
        &old_path,
        "Keep the moved decision.",
    )
    .await;
    let index = IndexService::new(state.clone());
    index.rebuild_vault(&core, &context).await.unwrap();
    let file = core.read(&context, &old_path).await.unwrap().file;
    core.move_entry(
        &context,
        &old_path,
        &new_path,
        file.current_revision,
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let move_report = SemanticMemoryService::new(state.clone())
        .reconcile_source_event(&context, &core, file.id)
        .await
        .unwrap();
    assert_eq!(
        move_report.disposition,
        mcp_vault_memory::SemanticSourceEventDisposition::NavigationOnly
    );
    let records = MemoryPackService::new(state.clone())
        .compare_abc(
            &context,
            &core,
            &index,
            &MemoryPackRequest {
                task: "moved decision".to_owned(),
                comparison_source_paths: vec!["notes/new-path.md".to_owned()],
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].mode, "A");
    assert_eq!(records[0].candidate_count, 0);
    assert_eq!(records[0].returned_count, 0);
    assert!(
        records[0]
            .gap_codes
            .iter()
            .any(|code| code == "ordinary_no_answer")
    );
    assert!(
        records[1..].iter().all(|record| {
            record.returned_count == 1
                && !record
                    .gap_codes
                    .iter()
                    .any(|code| code == "source_manifest_changed_before_response")
        }),
        "unexpected moved-path records: {records:#?}"
    );

    let moved_file = core.read(&context, &new_path).await.unwrap().file;
    let source = state
        .semantic_memory()
        .get_source_by_file(&context, moved_file.id)
        .await
        .unwrap()
        .unwrap();
    let source_revision = state
        .semantic_memory()
        .get_source_revision(&context, source.current_revision_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_ne!(source_revision.file_revision, moved_file.current_revision);
    assert_eq!(source.source_path, new_path);
    assert_eq!(
        source_revision.content_hash,
        moved_file.content_hash.unwrap()
    );
    let pack = MemoryPackService::new(state)
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "moved decision".to_owned(),
                comparison_source_paths: vec![new_path.to_string()],
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(pack.current_context.len(), 1);
    assert!(
        pack.current_context[0]
            .source_references
            .iter()
            .all(|reference| { reference.source_path.as_deref() == Some(new_path.as_str()) })
    );
    assert!(
        pack.current_context[0]
            .source_references
            .iter()
            .all(|reference| { reference.source_path.as_deref() != Some(old_path.as_str()) })
    );
}

#[tokio::test]
async fn memory_pack_abc_rejects_manifest_external_entry_before_budget_trim() {
    let (_dir, state, context, core) = fixture("pack-abc-manifest-external").await;
    let allowed_path = VaultPath::parse("notes/allowed.md").unwrap();
    let outside_path = VaultPath::parse("notes/outside.md").unwrap();
    for (path, statement) in [
        (&allowed_path, "Keep the allowed decision."),
        (&outside_path, "Keep the outside decision."),
    ] {
        core.create_bytes(
            &context,
            path,
            format!("# Decision\n{statement}\n").as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
        publish(&state, &context, &core, path, statement).await;
    }
    let index = IndexService::new(state.clone());
    index.rebuild_vault(&core, &context).await.unwrap();
    let records = MemoryPackService::new(state)
        .compare_abc(
            &context,
            &core,
            &index,
            &MemoryPackRequest {
                task: "outside decision".to_owned(),
                comparison_source_paths: vec!["notes/allowed.md".to_owned()],
                max_bytes: 64,
                max_tokens: 16,
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(records.iter().all(|record| {
        record.returned_count == 0
            && record
                .gap_codes
                .iter()
                .any(|code| code == "source_manifest_changed_before_response")
    }));
}

#[tokio::test]
async fn memory_pack_abc_manifest_is_vault_scoped() {
    let (_first_dir, first_state, first, first_core) = fixture("pack-abc-vault-first").await;
    let (_second_dir, second_state, second, second_core) = fixture("pack-abc-vault-second").await;
    let foreign_path = VaultPath::parse("notes/foreign.md").unwrap();
    second_core
        .create_bytes(
            &second,
            &foreign_path,
            b"# Foreign\nKeep only in the second Vault.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    publish(
        &second_state,
        &second,
        &second_core,
        &foreign_path,
        "Keep only in the second Vault.",
    )
    .await;
    let first_index = IndexService::new(first_state.clone());
    let records = MemoryPackService::new(first_state)
        .compare_abc(
            &first,
            &first_core,
            &first_index,
            &MemoryPackRequest {
                task: "foreign Vault".to_owned(),
                comparison_source_paths: vec![foreign_path.to_string()],
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(records.iter().all(|record| {
        record
            .gap_codes
            .iter()
            .any(|code| code == "comparison_source_manifest_invalid")
    }));
}

#[tokio::test]
async fn memory_pack_rechecks_after_real_source_qualification_changes() {
    let (_dir, state, context, core) = fixture("pack-recheck").await;
    let path = VaultPath::parse("notes/recheck.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Recheck\nKeep the recheck decision.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let card = publish(&state, &context, &core, &path, "Keep the recheck decision.").await;
    let (before, _) = state
        .list_memory_pack_candidates(&context, 200)
        .await
        .unwrap();
    assert!(before.iter().any(|candidate| candidate.id == card.id));
    let file = core.read(&context, &path).await.unwrap().file;
    let changed = core
        .replace_bytes(
            &context,
            &path,
            file.current_revision,
            b"# Recheck\nThe decision changed and must rebuild.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    SemanticMemoryService::new(state.clone())
        .reconcile_source_event(&context, &core, changed.file.id)
        .await
        .unwrap();
    let (after, _) = state
        .list_memory_pack_candidates(&context, 200)
        .await
        .unwrap();
    assert!(after.is_empty());
    let pack = MemoryPackService::new(state)
        .build(
            &context,
            &core,
            &MemoryPackRequest {
                task: "recheck decision".to_owned(),
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(pack.current_context.is_empty());
    assert!(pack.evidence_gaps.iter().any(|gap| gap.code == "no_answer"));
}

#[test]
fn memory_pack_request_schema_matches_existing_budget_limits() {
    let schema = serde_json::to_value(schemars::schema_for!(MemoryPackRequest)).unwrap();
    let properties = &schema["properties"];
    assert_eq!(properties["max_entries"]["minimum"].as_u64(), Some(1));
    assert_eq!(properties["max_entries"]["maximum"].as_u64(), Some(200));
    assert_eq!(properties["max_bytes"]["minimum"].as_u64(), Some(1));
    assert!(properties["max_bytes"]["maximum"].is_null());
    assert_eq!(properties["max_tokens"]["minimum"].as_u64(), Some(1));
    assert!(properties["max_tokens"]["maximum"].is_null());
}
