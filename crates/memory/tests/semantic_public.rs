use std::path::PathBuf;

use mcp_vault_core::VaultCore;
use mcp_vault_domain::{
    Actor, Permission, PermissionSet, Revision, SourcePlane, VaultContext, VaultId, VaultPath,
    VaultPathPolicy, VaultSlug, WritePrecondition,
};
use mcp_vault_memory::semantic::organize::SemanticOrganizationService;
use mcp_vault_memory::{
    MemoryPackRequest, SemanticAccess, SemanticActor, SemanticMemoryService, SemanticMutation,
    SemanticPublicFacade, SemanticRuleCommand,
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
        PathBuf::from(dir.path()).join("vault"),
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

async fn file_backed_fixture(
    slug: &str,
) -> (
    tempfile::TempDir,
    String,
    StateStore,
    VaultContext,
    VaultCore,
) {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("semantic-public.sqlite3");
    let url = format!("sqlite://{}", database.display());
    let state = StateStore::connect_and_migrate(&url).await.unwrap();
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
    let core = core_for(&state, &dir);
    (dir, url, state, context, core)
}

fn core_for(state: &StateStore, dir: &tempfile::TempDir) -> VaultCore {
    VaultCore::new(
        state.clone(),
        dir.path().join("history"),
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
        Default::default(),
    )
}

async fn secondary_vault(
    state: &StateStore,
    dir: &tempfile::TempDir,
    slug: &str,
) -> (VaultContext, VaultCore) {
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new(slug).unwrap(),
        dir.path().join(slug),
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
    let core = core_for(state, dir);
    (context, core)
}

#[tokio::test]
async fn public_facade_reads_composed_cards_and_rejects_stale_parent_revision() {
    let (_dir, state, context, core) = fixture("public-composed").await;
    let first_path = VaultPath::parse("notes/first.md").unwrap();
    let second_path = VaultPath::parse("notes/second.md").unwrap();
    let first = publish(&state, &context, &core, &first_path).await;
    let second = publish(&state, &context, &core, &second_path).await;
    let organization = SemanticOrganizationService::new(state.clone());
    let candidate = organization
        .discover_candidates(&context, &[first.source_id, second.source_id])
        .await
        .unwrap()
        .pop()
        .unwrap();
    organization
        .organize_json(
            &context,
            &core,
            &[first.source_id, second.source_id],
            &json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":"public-composed","title":"Public composed","support_operator":"or"}]}).to_string(),
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
    let facade = SemanticPublicFacade::new(state.clone());
    let read = facade
        .get_composed_card(
            &context,
            &core,
            &access(&[Permission::ReadMemory, Permission::ReadVault]),
            &composed.id.to_string(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.composed_card_id, composed.id.to_string());
    let actor = SemanticActor::trusted("server:composed");
    let stale = SemanticRuleCommand {
        target_ref: format!("composed_card:{}", composed.id),
        parent_ref: None,
        source_id: None,
        source_revision_id: None,
        mutation: SemanticMutation::ForgetCurrent,
        payload: Some(json!({"reason":"stale parent test"})),
        expected_parent_revision: Some(composed.revision_number + 1),
        expected_rules_revision: Some(0),
        idempotency_key: "composed-stale-parent".to_owned(),
    };
    assert!(matches!(
        facade
            .apply_rule(
                &context,
                &access(&[Permission::ManageMemory]),
                &actor,
                &stale,
            )
            .await,
        Err(mcp_vault_memory::MemoryError::Conflict)
    ));
    let command = SemanticRuleCommand {
        expected_parent_revision: Some(composed.revision_number),
        expected_rules_revision: Some(0),
        idempotency_key: "composed-target-level".to_owned(),
        ..stale
    };
    let result = facade
        .apply_rule(
            &context,
            &access(&[Permission::ManageMemory]),
            &actor,
            &command,
        )
        .await
        .unwrap();
    assert_eq!(result.action, "forget_current");
}

async fn publish(
    state: &StateStore,
    context: &VaultContext,
    core: &VaultCore,
    path: &VaultPath,
) -> mcp_vault_state::SemanticCardRecord {
    core.create_bytes(
        context,
        path,
        b"# Decision\nKeep the public facade safe.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let memory = SemanticMemoryService::new(state.clone());
    let input = memory.prepare_source(context, core, path).await.unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":"Keep the public facade safe.","scope":"project","assertion_status":"source_asserted","conditions":[],"exceptions":[],"ordered_steps":[],"admission_reason":"test","value_for_future_work":"test","body_block_ids":[body.local_id.clone()]}],
        "cards":[{"title":"Public facade","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
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

async fn publish_cards(
    state: &StateStore,
    context: &VaultContext,
    core: &VaultCore,
    path: &VaultPath,
    statement: &str,
    titles: &[&str],
    create_file: bool,
) -> Vec<mcp_vault_state::SemanticCardRecord> {
    if create_file {
        core.create_bytes(
            context,
            path,
            b"# Decision\nKeep the public facade safe.\nThe public facade remains safe.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    }
    let memory = SemanticMemoryService::new(state.clone());
    let input = memory.prepare_source(context, core, path).await.unwrap();
    let body = input.blocks.last().unwrap();
    let cards = titles
        .iter()
        .map(|title| {
            json!({
                "title": title,
                "kind": "decision",
                "scope": "project",
                "assertion_status": "source_asserted",
                "observation_indices": [0]
            })
        })
        .collect::<Vec<_>>();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":statement,"scope":"project","assertion_status":"source_asserted","conditions":[],"exceptions":[],"ordered_steps":[],"admission_reason":"test","value_for_future_work":"test","body_block_ids":[body.local_id.clone()]}],
        "cards":cards
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
}

#[allow(clippy::too_many_arguments)]
async fn publish_observation_set(
    state: &StateStore,
    context: &VaultContext,
    core: &VaultCore,
    path: &VaultPath,
    content: &str,
    statements: &[&str],
    card_title: &str,
    card_observation_indices: &[u32],
    create_file: bool,
) -> (
    mcp_vault_state::SemanticExtractionRecord,
    Vec<mcp_vault_state::SemanticCardRecord>,
) {
    if create_file {
        core.create_bytes(
            context,
            path,
            content.as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    }
    let memory = SemanticMemoryService::new(state.clone());
    let input = memory.prepare_source(context, core, path).await.unwrap();
    let body = input.blocks.last().unwrap();
    let observations = statements
        .iter()
        .map(|statement| {
            json!({
                "kind":"decision",
                "statement":statement,
                "scope":"project",
                "assertion_status":"source_asserted",
                "conditions":[],
                "exceptions":[],
                "ordered_steps":[],
                "admission_reason":"test",
                "value_for_future_work":"test",
                "body_block_ids":[body.local_id.clone()]
            })
        })
        .collect::<Vec<_>>();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":observations,
        "cards":[{"title":card_title,"kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":card_observation_indices}]
    });
    let submission = memory
        .submit_proposal_json(context, core, path, &proposal.to_string())
        .await
        .unwrap();
    let cards = state
        .semantic_memory()
        .list_cards(context, 20)
        .await
        .unwrap()
        .into_iter()
        .filter(|card| card.source_path == *path)
        .collect();
    (submission.extraction, cards)
}

fn rule_command(
    target_ref: String,
    parent_ref: Option<String>,
    mutation: SemanticMutation,
    expected_parent_revision: Option<u32>,
    expected_rules_revision: i64,
    idempotency_key: &str,
) -> SemanticRuleCommand {
    let payload = match &mutation {
        SemanticMutation::Correction => {
            json!({"replace":"The public facade remains safe after review.","remove":"The public facade remains safe."})
        }
        _ => json!({"reason":"public semantic regression"}),
    };
    SemanticRuleCommand {
        target_ref,
        parent_ref,
        source_id: None,
        source_revision_id: None,
        mutation,
        payload: Some(payload),
        expected_parent_revision,
        expected_rules_revision: Some(expected_rules_revision),
        idempotency_key: idempotency_key.to_owned(),
    }
}

#[tokio::test]
async fn file_backed_historical_card_evidence_is_not_resolvable_or_mutable() {
    let (dir, url, state, context, core) = file_backed_fixture("public-historical-card").await;
    let path = VaultPath::parse("notes/historical.md").unwrap();
    let first = publish_cards(
        &state,
        &context,
        &core,
        &path,
        "Keep the public facade safe.",
        &["Public facade"],
        true,
    )
    .await;
    let historical_evidence = first[0].items[0].evidence_ref_ids[0];
    let historical_card = first[0].id;
    let current = publish_cards(
        &state,
        &context,
        &core,
        &path,
        "The public facade remains safe.",
        &["Public facade"],
        false,
    )
    .await;
    assert_eq!(current.len(), 1);
    assert_ne!(current[0].revision_id, first[0].revision_id);
    assert_eq!(current[0].id, historical_card);

    drop(core);
    drop(state);
    let reopened = StateStore::connect_and_migrate(&url).await.unwrap();
    let reopened_core = core_for(&reopened, &dir);
    let evidence_ref = format!("evidence:{historical_evidence}");
    assert!(
        reopened
            .semantic_rules()
            .resolve_semantic_target(&context, &evidence_ref)
            .await
            .unwrap()
            .is_none()
    );

    let error = SemanticPublicFacade::new(reopened)
        .apply_rule(
            &context,
            &access(&[Permission::ManageMemory]),
            &SemanticActor::trusted("file-backed:test"),
            &rule_command(
                evidence_ref,
                Some(format!("card:{historical_card}")),
                SemanticMutation::SuppressRead,
                Some(current[0].revision_number),
                0,
                "historical-card-evidence",
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, mcp_vault_memory::MemoryError::NotFound));
    assert!(reopened_core.read(&context, &path).await.is_ok());
}

#[tokio::test]
async fn file_backed_current_evidence_requires_an_explicit_unambiguous_parent() {
    let (dir, url, state, context, core) = file_backed_fixture("public-multi-parent").await;
    let first_path = VaultPath::parse("notes/first.md").unwrap();
    let second_path = VaultPath::parse("notes/second.md").unwrap();
    let first = publish(&state, &context, &core, &first_path).await;
    let second = publish(&state, &context, &core, &second_path).await;
    let organization = SemanticOrganizationService::new(state.clone());
    let candidate = organization
        .discover_candidates(&context, &[first.source_id, second.source_id])
        .await
        .unwrap()
        .pop()
        .unwrap();
    organization
        .organize_json(
            &context,
            &core,
            &[first.source_id, second.source_id],
            &json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":"multi-parent","title":"Multi parent","support_operator":"or"}]}).to_string(),
        )
        .await
        .unwrap();
    let current_composed = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let current_evidence = first.items[0].evidence_ref_ids[0];

    let (foreign_context, foreign_core) = secondary_vault(&state, &dir, "public-foreign").await;
    let foreign_path = VaultPath::parse("notes/foreign.md").unwrap();
    let foreign = publish_cards(
        &state,
        &foreign_context,
        &foreign_core,
        &foreign_path,
        "Keep the public facade safe.",
        &["Foreign"],
        true,
    )
    .await;

    let current_ref = format!("evidence:{current_evidence}");
    assert!(matches!(
        state
            .semantic_rules()
            .resolve_semantic_target(&context, &current_ref)
            .await,
        Err(mcp_vault_state::StateError::Conflict)
    ));
    let facade = SemanticPublicFacade::new(state.clone());
    let actor = SemanticActor::trusted("file-backed:multi-parent");
    let ambiguous = facade
        .apply_rule(
            &context,
            &access(&[Permission::ManageMemory]),
            &actor,
            &rule_command(
                current_ref.clone(),
                None,
                SemanticMutation::SuppressRegeneration,
                None,
                0,
                "multi-parent-ambiguous",
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(ambiguous, mcp_vault_memory::MemoryError::Conflict));

    let parent = &first;
    let accepted = facade
        .apply_rule(
            &context,
            &access(&[Permission::ManageMemory]),
            &actor,
            &rule_command(
                current_ref.clone(),
                Some(format!("card:{}", parent.id)),
                SemanticMutation::SuppressRegeneration,
                Some(parent.revision_number),
                0,
                "multi-parent-explicit",
            ),
        )
        .await
        .unwrap();
    assert_eq!(accepted.action, "suppress_regeneration");

    let composed_accepted = facade
        .apply_rule(
            &context,
            &access(&[Permission::ManageMemory]),
            &actor,
            &rule_command(
                current_ref.clone(),
                Some(format!("composed_card:{}", current_composed.id)),
                SemanticMutation::SuppressRegeneration,
                Some(current_composed.revision_number),
                1,
                "multi-parent-composed-explicit",
            ),
        )
        .await
        .unwrap();
    assert_eq!(composed_accepted.action, "suppress_regeneration");

    let foreign_parent = format!("card:{}", foreign[0].id);
    assert!(
        state
            .semantic_rules()
            .resolve_semantic_target_for_parent(&context, &current_ref, &foreign_parent)
            .await
            .unwrap()
            .is_none()
    );
    let rejected = facade
        .apply_rule(
            &context,
            &access(&[Permission::ManageMemory]),
            &actor,
            &rule_command(
                current_ref,
                Some(foreign_parent),
                SemanticMutation::SuppressRead,
                None,
                1,
                "multi-parent-foreign",
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(rejected, mcp_vault_memory::MemoryError::NotFound));

    drop(foreign_core);
    drop(core);
    drop(state);
    let reopened = StateStore::connect_and_migrate(&url).await.unwrap();
    assert_eq!(
        reopened
            .semantic_rules()
            .current_rules_revision(&context)
            .await
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn file_backed_composed_target_keeps_source_scoped_rules_to_real_supports() {
    let (dir, url, state, context, core) = file_backed_fixture("public-composed-supports").await;
    let first_path = VaultPath::parse("notes/first.md").unwrap();
    let second_path = VaultPath::parse("notes/second.md").unwrap();
    let third_path = VaultPath::parse("notes/third.md").unwrap();
    let first = publish(&state, &context, &core, &first_path).await;
    let second = publish(&state, &context, &core, &second_path).await;
    let third = publish(&state, &context, &core, &third_path).await;
    let organization = SemanticOrganizationService::new(state.clone());
    let candidate = organization
        .discover_candidates(&context, &[first.source_id, second.source_id])
        .await
        .unwrap()
        .pop()
        .unwrap();
    organization
        .organize_json(
            &context,
            &core,
            &[first.source_id, second.source_id],
            &json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":"support-target","title":"Support target","support_operator":"or"}]}).to_string(),
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
    let target_ref = format!("composed_card:{}", composed.id);
    let resolved = state
        .semantic_rules()
        .resolve_semantic_target(&context, &target_ref)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.source_bindings.len(), 2);
    assert!(
        resolved
            .source_bindings
            .iter()
            .any(|binding| binding.source_id == first.source_id.to_string())
    );
    assert!(
        resolved
            .source_bindings
            .iter()
            .any(|binding| binding.source_id == second.source_id.to_string())
    );
    assert!(
        !resolved
            .source_bindings
            .iter()
            .any(|binding| binding.source_id == third.source_id.to_string())
    );

    let facade = SemanticPublicFacade::new(state.clone());
    let actor = SemanticActor::trusted("file-backed:composed-supports");
    let target_level = facade
        .apply_rule(
            &context,
            &access(&[Permission::ManageMemory]),
            &actor,
            &rule_command(
                target_ref.clone(),
                None,
                SemanticMutation::Correction,
                Some(composed.revision_number),
                0,
                "composed-target-correction",
            ),
        )
        .await
        .unwrap();
    assert_eq!(target_level.action, "replace");

    let source_scoped = SemanticRuleCommand {
        target_ref: target_ref.clone(),
        parent_ref: None,
        source_id: Some(first.source_id.to_string()),
        source_revision_id: Some(first.source_revision_id.to_string()),
        mutation: SemanticMutation::SuppressRegeneration,
        payload: Some(json!({"reason":"supported source only"})),
        expected_parent_revision: Some(composed.revision_number),
        expected_rules_revision: Some(1),
        idempotency_key: "composed-source-scoped".to_owned(),
    };
    let source_result = facade
        .apply_rule(
            &context,
            &access(&[Permission::ManageMemory]),
            &actor,
            &source_scoped,
        )
        .await
        .unwrap();
    assert_eq!(source_result.action, "suppress_regeneration");

    let wrong_source = SemanticRuleCommand {
        source_id: Some(third.source_id.to_string()),
        source_revision_id: Some(third.source_revision_id.to_string()),
        expected_rules_revision: Some(2),
        idempotency_key: "composed-wrong-source".to_owned(),
        ..source_scoped
    };
    let error = facade
        .apply_rule(
            &context,
            &access(&[Permission::ManageMemory]),
            &actor,
            &wrong_source,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, mcp_vault_memory::MemoryError::Conflict));

    drop(core);
    drop(state);
    let reopened = StateStore::connect_and_migrate(&url).await.unwrap();
    assert_eq!(
        reopened
            .semantic_rules()
            .current_rules_revision(&context)
            .await
            .unwrap(),
        2
    );
    let reopened_core = core_for(&reopened, &dir);
    assert!(
        SemanticPublicFacade::new(reopened)
            .get_composed_card(
                &context,
                &reopened_core,
                &access(&[Permission::ReadMemory, Permission::ReadVault]),
                &composed.id.to_string(),
            )
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn composed_mutation_rejects_when_all_or_supports_are_stale() {
    let (_dir, state, context, core) = fixture("public-composed-stale-support").await;
    let first_path = VaultPath::parse("notes/first.md").unwrap();
    let second_path = VaultPath::parse("notes/second.md").unwrap();
    let first = publish(&state, &context, &core, &first_path).await;
    let second = publish(&state, &context, &core, &second_path).await;
    let organization = SemanticOrganizationService::new(state.clone());
    let candidate = organization
        .discover_candidates(&context, &[first.source_id, second.source_id])
        .await
        .unwrap()
        .pop()
        .unwrap();
    organization
        .organize_json(
            &context,
            &core,
            &[first.source_id, second.source_id],
            &json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":"stale-support","title":"Stale support","support_operator":"or"}]}).to_string(),
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
    for source_id in [first.source_id, second.source_id] {
        let source = state
            .semantic_memory()
            .get_source(&context, source_id)
            .await
            .unwrap()
            .unwrap();
        state
            .semantic_memory()
            .invalidate_source(&context, source.file_id, "source_changed", None)
            .await
            .unwrap();
    }
    let error = SemanticPublicFacade::new(state)
        .apply_rule(
            &context,
            &access(&[Permission::ManageMemory]),
            &SemanticActor::trusted("stale-support-test"),
            &rule_command(
                format!("composed_card:{}", composed.id),
                None,
                SemanticMutation::Correction,
                Some(composed.revision_number),
                0,
                "stale-composed-target",
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, mcp_vault_memory::MemoryError::NotFound));
}

#[tokio::test]
async fn composed_mutation_rejects_when_and_support_is_stale() {
    let (_dir, state, context, core) = fixture("public-composed-stale-and").await;
    let first_path = VaultPath::parse("notes/first.md").unwrap();
    let second_path = VaultPath::parse("notes/second.md").unwrap();
    let first = publish(&state, &context, &core, &first_path).await;
    let second = publish(&state, &context, &core, &second_path).await;
    let organization = SemanticOrganizationService::new(state.clone());
    let candidate = organization
        .discover_candidates(&context, &[first.source_id, second.source_id])
        .await
        .unwrap()
        .pop()
        .unwrap();
    organization
        .organize_json(
            &context,
            &core,
            &[first.source_id, second.source_id],
            &json!({"actions":[{"action":"create_composed_card","candidate_ids":[candidate.id.to_string()],"card_ref":"stale-and","title":"Stale AND","support_operator":"and"}]}).to_string(),
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
    let source = state
        .semantic_memory()
        .get_source(&context, first.source_id)
        .await
        .unwrap()
        .unwrap();
    state
        .semantic_memory()
        .invalidate_source(&context, source.file_id, "source_changed", None)
        .await
        .unwrap();
    let error = SemanticPublicFacade::new(state)
        .apply_rule(
            &context,
            &access(&[Permission::ManageMemory]),
            &SemanticActor::trusted("stale-and-test"),
            &rule_command(
                format!("composed_card:{}", composed.id),
                None,
                SemanticMutation::Correction,
                Some(composed.revision_number),
                0,
                "stale-and-target",
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, mcp_vault_memory::MemoryError::NotFound));
}

#[tokio::test]
async fn file_backed_evidence_from_historical_composed_revision_is_not_resolvable() {
    let (_dir, url, state, context, core) = file_backed_fixture("public-historical-composed").await;
    let first_path = VaultPath::parse("notes/first.md").unwrap();
    let second_path = VaultPath::parse("notes/second.md").unwrap();
    let third_path = VaultPath::parse("notes/third.md").unwrap();
    let content = "# Decision\nShared decision.\n";
    let (_first_extraction, first_cards) = publish_observation_set(
        &state,
        &context,
        &core,
        &first_path,
        content,
        &["Shared decision."],
        "First card",
        &[0],
        true,
    )
    .await;
    let (_second_extraction, second_cards) = publish_observation_set(
        &state,
        &context,
        &core,
        &second_path,
        content,
        &["Shared decision.", "Shared decision."],
        "Second card",
        &[0, 1],
        true,
    )
    .await;
    let (_third_extraction, third_cards) = publish_observation_set(
        &state,
        &context,
        &core,
        &third_path,
        content,
        &["Shared decision."],
        "Third card",
        &[0],
        true,
    )
    .await;
    let first = &first_cards[0];
    let second = &second_cards[0];
    let third = &third_cards[0];
    let second_observations = state
        .semantic_memory()
        .list_observations(&context, _second_extraction.id)
        .await
        .unwrap();
    let historical_observation = &second_observations[1];
    let historical_evidence = historical_observation.evidence_ref_id;
    let organization = SemanticOrganizationService::new(state.clone());
    let first_candidates = organization
        .discover_candidates(
            &context,
            &[first.source_id, second.source_id, third.source_id],
        )
        .await
        .unwrap();
    let old_candidate = first_candidates
        .iter()
        .find(|candidate| {
            candidate.left.source_id == first.source_id
                && candidate.right.source_id == second.source_id
                && candidate.right.id == historical_observation.id
        })
        .unwrap();
    organization
        .organize_json(
            &context,
            &core,
            &[first.source_id, second.source_id, third.source_id],
            &json!({"actions":[{"action":"create_composed_card","candidate_ids":[old_candidate.id.to_string()],"card_ref":"historical-composed","title":"Historical composed","support_operator":"or"}]}).to_string(),
        )
        .await
        .unwrap();
    let old_composed = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert!(
        old_composed
            .items
            .iter()
            .flat_map(|item| item.support_groups.iter())
            .flat_map(|group| group.members.iter())
            .any(|member| member.evidence_ref_id == historical_evidence)
    );

    let new_candidate = first_candidates
        .iter()
        .find(|candidate| {
            candidate.left.source_id == first.source_id
                && candidate.right.source_id == third.source_id
        })
        .unwrap();
    organization
        .organize_json(
            &context,
            &core,
            &[first.source_id, second.source_id, third.source_id],
            &json!({"actions":[{"action":"create_composed_card","candidate_ids":[new_candidate.id.to_string()],"card_ref":"historical-composed","title":"Current composed","support_operator":"or"}]}).to_string(),
        )
        .await
        .unwrap();
    let current_composed = state
        .semantic_organization()
        .list_composed_cards(&context, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(current_composed.id, old_composed.id);
    assert_ne!(current_composed.revision_id, old_composed.revision_id);
    assert!(
        !current_composed
            .items
            .iter()
            .flat_map(|item| item.support_groups.iter())
            .flat_map(|group| group.members.iter())
            .any(|member| member.evidence_ref_id == historical_evidence)
    );

    let (_current_second_extraction, current_second_cards) = publish_observation_set(
        &state,
        &context,
        &core,
        &second_path,
        content,
        &["Shared decision."],
        "Second card",
        &[0],
        false,
    )
    .await;
    assert!(
        !current_second_cards
            .iter()
            .flat_map(|card| card.items.iter())
            .flat_map(|item| item.evidence_ref_ids.iter())
            .any(|evidence| *evidence == historical_evidence)
    );

    drop(core);
    drop(state);
    let reopened = StateStore::connect_and_migrate(&url).await.unwrap();
    let evidence_ref = format!("evidence:{historical_evidence}");
    assert!(
        reopened
            .semantic_rules()
            .resolve_semantic_target(&context, &evidence_ref)
            .await
            .unwrap()
            .is_none()
    );
    let error = SemanticPublicFacade::new(reopened)
        .apply_rule(
            &context,
            &access(&[Permission::ManageMemory]),
            &SemanticActor::trusted("file-backed:historical-composed"),
            &rule_command(
                evidence_ref,
                Some(format!("composed_card:{}", old_composed.id)),
                SemanticMutation::SuppressRead,
                Some(current_composed.revision_number),
                0,
                "historical-composed-evidence",
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, mcp_vault_memory::MemoryError::NotFound));
}

fn access(permissions: &[Permission]) -> SemanticAccess {
    SemanticAccess::new(permissions.iter().copied().collect::<PermissionSet>())
}

#[tokio::test]
async fn public_facade_requires_vault_and_memory_read_and_lists_safe_cards() {
    let (_dir, state, context, core) = fixture("public-facade").await;
    let path = VaultPath::parse("notes/public.md").unwrap();
    let card = publish(&state, &context, &core, &path).await;
    let facade = SemanticPublicFacade::new(state);
    let denied = facade
        .build_pack(
            &context,
            &core,
            &access(&[Permission::ReadMemory]),
            &MemoryPackRequest {
                task: "public facade".to_owned(),
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(denied.code(), "memory_access_denied");
    let (cards, composed) = facade
        .list_cards(
            &context,
            &core,
            &access(&[Permission::ReadMemory, Permission::ReadVault]),
            20,
        )
        .await
        .unwrap();
    assert_eq!(composed.len(), 0);
    assert_eq!(cards[0].card_id, card.id.to_string());
    assert!(cards[0].items[0].content.contains("public facade"));
}

#[tokio::test]
async fn public_facade_resolves_card_rules_without_deleting_source_or_crossing_vaults() {
    let (_dir, state, context, core) = fixture("public-rules").await;
    let (_other_dir, other_state, other_context, _other_core) = fixture("other-rules").await;
    let path = VaultPath::parse("notes/public.md").unwrap();
    let card = publish(&state, &context, &core, &path).await;
    let facade = SemanticPublicFacade::new(state.clone());
    let actor = SemanticActor::trusted("server:test");
    let command = SemanticRuleCommand {
        target_ref: format!("card:{}", card.id),
        parent_ref: None,
        source_id: None,
        source_revision_id: None,
        mutation: SemanticMutation::SuppressRead,
        payload: Some(json!({"reason":"public facade test"})),
        expected_parent_revision: Some(card.revision_number),
        expected_rules_revision: Some(0),
        idempotency_key: "public-suppress-once".to_owned(),
    };
    let first = facade
        .apply_rule(
            &context,
            &access(&[Permission::ManageMemory]),
            &actor,
            &command,
        )
        .await
        .unwrap();
    let replay = facade
        .apply_rule(
            &context,
            &access(&[Permission::ManageMemory]),
            &actor,
            &command,
        )
        .await
        .unwrap();
    assert_eq!(first.id, replay.id);
    assert!(core.read(&context, &path).await.is_ok());
    assert!(
        other_state
            .semantic_rules()
            .resolve_semantic_target(&other_context, &command.target_ref)
            .await
            .unwrap()
            .is_none()
    );
}
